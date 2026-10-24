param(
 [Parameter(Mandatory=$true)][string]$InstallationDirectory,
 [string]$EvidenceDirectory,
 [string]$RecoveryScript=(Join-Path (Split-Path $PSScriptRoot -Parent) 'recover-supervisor.ps1')
)
$ErrorActionPreference='Stop'
$installation=[IO.Path]::GetFullPath($InstallationDirectory)
$root=if($EvidenceDirectory){[IO.Path]::GetFullPath($EvidenceDirectory)}else{Join-Path $installation '.audit\recovery-tests'}
$null=New-Item -ItemType Directory -Path $root -Force
$id=[guid]::NewGuid().ToString('N')
$directory=Join-Path $root ('persistence-'+$id)
$null=New-Item -ItemType Directory -Path $directory
$data=Join-Path $directory 'data';$null=New-Item -ItemType Directory -Path $data
$exe=Join-Path $directory 'ghost-reforged-supervisor.exe'
$cli=Join-Path $directory 'ghost-reforged-cli.exe'
foreach($name in @('ghost-reforged-supervisor.exe','ghost-reforged-cli.exe')){Copy-Item -LiteralPath (Join-Path $installation $name) -Destination $directory}
$settings=Join-Path $directory 'settings.json'
[IO.File]::WriteAllText($settings,'{"headless":true,"rehost":false}',(New-Object Text.UTF8Encoding($false)))
& $cli --data-dir $data profile-set --file $settings|Out-Null
if($LASTEXITCODE-ne 0){throw 'Cannot initialize isolated fixture'}
[IO.File]::WriteAllText((Join-Path $data 'hosting-intent.json'),'{"version":1,"hosting":false,"quit":false}',(New-Object Text.UTF8Encoding($false)))
$receipt=[ordered]@{started_utc=[DateTime]::UtcNow.ToString('o');isolated=$true;hosting_requested=$false;idle=$false;controller_crash_recovered=$false;scheduler_restarted_supervisor=$false;orphan_adopted=$false;quit_preserved=$false;passed=$false}
$receipt.binaries=@{}
foreach($name in @('ghost-reforged-supervisor.exe','ghost-reforged-cli.exe')){$receipt.binaries[$name]=(Get-FileHash -LiteralPath (Join-Path $directory $name)).Hash}
$schedule=New-Object -ComObject Schedule.Service;$schedule.Connect();$folder=$schedule.GetFolder('\')
$primary=$folder.GetTask('GHostReforged-Supervisor')
$name='GHostReforged-PersistenceTest-'+$id
$task=$null;$recoveryTask=$null;$parent=$null;$worker=$null
function Record { $receipt.utc=[DateTime]::UtcNow.ToString('o');$receipt|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $directory 'result.json') }
function Health {
 $saved=$ErrorActionPreference
 try{$ErrorActionPreference='Continue';$h=& $cli --data-dir $data health 2>$null|ConvertFrom-Json;if($LASTEXITCODE-eq 0){$h}}finally{$ErrorActionPreference=$saved}
}
function Wait-Health([int]$Previous=0,[int]$Seconds=50) {
 $deadline=[DateTime]::UtcNow.AddSeconds($Seconds)
 do{
  Start-Sleep -Milliseconds 500;$h=Health
  if($h-and $h.pid-ne $Previous){
   if($h.hosting_requested-or $h.worker){throw 'Fixture must remain idle'}
   return $h
  }
 }while([DateTime]::UtcNow-lt $deadline)
 throw 'Isolated controller did not become ready'
}
function Retain([int]$ProcessId){
 $p=Get-Process -Id $ProcessId -ErrorAction Stop;$null=$p.Handle
 if($p.Path-ine $exe-or $p.SessionId-eq 0){throw 'Fault target is not the isolated console fixture'}
 $p
}
try{
 $definition=$schedule.NewTask(0)
 $definition.Principal.UserId=$primary.Definition.Principal.UserId
 $definition.Principal.LogonType=$primary.Definition.Principal.LogonType
 $definition.Principal.RunLevel=$primary.Definition.Principal.RunLevel
 $definition.Settings.ExecutionTimeLimit='PT0S';$definition.Settings.MultipleInstances=0
 if(-not $RecoveryScript){
  $definition.Settings.RestartInterval=$primary.Definition.Settings.RestartInterval
  $definition.Settings.RestartCount=$primary.Definition.Settings.RestartCount
 }
 $definition.Settings.StartWhenAvailable=$true
 $definition.Settings.Priority=$primary.Definition.Settings.Priority
 $trigger=$definition.Triggers.Create(1);$trigger.StartBoundary=[DateTime]::Now.AddSeconds(5).ToString('s')
 $action=$definition.Actions.Create(0);$action.Path=$exe;$action.Arguments='--data-dir "'+$data+'"';$action.WorkingDirectory=$directory
 $task=$folder.RegisterTaskDefinition($name,$definition,2,$definition.Principal.UserId,$null,$definition.Principal.LogonType)
 if($RecoveryScript){
  $probe=Join-Path $directory 'recovery-probe.ps1'
  @'
param([string]$RecoveryScript,[string]$DataDirectory,[string]$Executable,[string]$TaskName)
$ErrorActionPreference='Stop'
$receiptPath=Join-Path $DataDirectory 'recovery-probe.json'
@{started_utc=[DateTime]::UtcNow.ToString('o');pid=$PID;complete=$false}|ConvertTo-Json|Set-Content -LiteralPath $receiptPath
try {
 & $RecoveryScript -DataDirectory $DataDirectory -Executable $Executable -TaskName $TaskName
 @{finished_utc=[DateTime]::UtcNow.ToString('o');pid=$PID;complete=$true}|ConvertTo-Json|Set-Content -LiteralPath $receiptPath
} catch {
 @{finished_utc=[DateTime]::UtcNow.ToString('o');pid=$PID;complete=$true;error=$_.Exception.Message;position=$_.InvocationInfo.PositionMessage}|ConvertTo-Json|Set-Content -LiteralPath $receiptPath
 exit 1
}
'@ | Set-Content -LiteralPath $probe
  $recovery=$schedule.NewTask(0);$recovery.Principal.UserId=$definition.Principal.UserId;$recovery.Principal.LogonType=$definition.Principal.LogonType;$recovery.Principal.RunLevel=$definition.Principal.RunLevel
  $recovery.Settings.ExecutionTimeLimit='PT45S';$recovery.Settings.MultipleInstances=2;$recovery.Settings.StartWhenAvailable=$true
  $rt=$recovery.Triggers.Create(1);$rt.StartBoundary=[DateTime]::Now.AddSeconds(20).ToString('s');$rt.Repetition.Interval='PT1M'
  $ra=$recovery.Actions.Create(0);$ra.Path=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
  $ra.Arguments='-NoProfile -NonInteractive -WindowStyle Hidden -File "'+$probe+'" -RecoveryScript "'+$RecoveryScript+'" -DataDirectory "'+$data+'" -Executable "'+$exe+'" -TaskName "'+$name+'"'
  $ra.WorkingDirectory=$directory
  $recoveryTask=$folder.RegisterTaskDefinition(($name+'-Recovery'),$recovery,2,$recovery.Principal.UserId,$null,$recovery.Principal.LogonType)
  $receipt.recovery_sha256=(Get-FileHash -LiteralPath $RecoveryScript).Hash
 }
 $receipt.task=$name;Record
 @{directory=[IO.Path]::GetFileName($directory);task=$name}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $root 'persistence-latest.json')
 $health=Wait-Health;$worker=Retain $health.pid
 $state=Get-Content -Raw -LiteralPath (Join-Path $data 'supervisor-status.json')|ConvertFrom-Json
 $parent=Retain $state.supervisor_pid
 $receipt.idle=$true;Record
 $previousWorker=$worker.Id;$worker.Kill();if(-not $worker.WaitForExit(5000)){throw 'Fixture controller exit unconfirmed'}
 $health=Wait-Health $previousWorker;$worker=Retain $health.pid
 $receipt.controller_crash_recovered=$true;Record
 $previousParent=$parent.Id;$retainedWorker=$worker.Id
 $timer=[Diagnostics.Stopwatch]::StartNew()
 $parent.Kill();if(-not $parent.WaitForExit(5000)){throw 'Fixture supervisor exit unconfirmed'}
 $receipt.task_state_after_parent_exit=$folder.GetTask($name).State;Record
 $deadline=[DateTime]::UtcNow.AddSeconds(150)
 do{
  Start-Sleep -Seconds 2
  $state=Get-Content -Raw -LiteralPath (Join-Path $data 'supervisor-status.json')|ConvertFrom-Json
  if($state.supervisor_pid-ne $previousParent){break}
 }while([DateTime]::UtcNow-lt $deadline)
 if($state.supervisor_pid-eq $previousParent){throw 'Windows did not restart the failed supervisor'}
 $parent=Retain $state.supervisor_pid
 $receipt.scheduler_restarted_supervisor=$true;$receipt.scheduler_recovery_seconds=$timer.Elapsed.TotalSeconds
 $health=Wait-Health
 if($health.pid-ne $retainedWorker){throw 'Surviving controller was replaced instead of adopted'}
 $receipt.orphan_adopted=$true;Record
 if($recoveryTask){
  $previousParent=$parent.Id;$parent.Kill();if(-not $parent.WaitForExit(5000)){throw 'Repeated supervisor fault did not exit'}
  $timer.Restart();$deadline=[DateTime]::UtcNow.AddSeconds(100)
  do{Start-Sleep -Seconds 2;$state=Get-Content -Raw -LiteralPath (Join-Path $data 'supervisor-status.json')|ConvertFrom-Json}while($state.supervisor_pid-eq $previousParent-and [DateTime]::UtcNow-lt $deadline)
  if($state.supervisor_pid-eq $previousParent){throw 'Recurring recovery failed'}
  $parent=Retain $state.supervisor_pid
  if((Wait-Health).pid-ne $retainedWorker){throw 'Repeated recovery replaced the surviving controller'}
  $receipt.repeated_recovery_seconds=$timer.Elapsed.TotalSeconds
  $null=$recoveryTask.Run($null);Start-Sleep -Seconds 5
  $active=@(Get-Process -Name 'ghost-reforged-supervisor' -ErrorAction SilentlyContinue|Where-Object {$_.Path-ieq $exe})
  if($active.Count-ne 2){throw 'Live recovery check duplicated a process'}
  $receipt.concurrent_check_excluded=$true;Record
 }
 & $cli --data-dir $data quit|Out-Null
 if($LASTEXITCODE-ne 0){throw 'Fixture Quit request failed'}
 if(-not $parent.WaitForExit(30000)-or $parent.ExitCode-ne 0){throw 'Fixture Quit did not close supervisor cleanly'}
 Start-Sleep -Seconds 65
 $intent=Get-Content -Raw -LiteralPath (Join-Path $data 'hosting-intent.json')|ConvertFrom-Json
 $remaining=@(Get-Process -Name 'ghost-reforged-supervisor' -ErrorAction SilentlyContinue|Where-Object {$_.Path-ieq $exe})
 if($remaining.Count-or -not $intent.quit-or $intent.hosting){throw 'Windows lost the explicit Quit intent'}
 $receipt.quit_preserved=$true;$receipt.passed=$true;Record
}catch{$receipt.error=$_.Exception.Message;Record}
finally{
 if($recoveryTask){
  $receipt.recovery_last_result=$recoveryTask.LastTaskResult
  $receipt.recovery_last_run=$recoveryTask.LastRunTime.ToString('o')
  $receipt.recovery_state=$recoveryTask.State
  $probeReceipt=Join-Path $data 'recovery-probe.json'
  if(Test-Path -LiteralPath $probeReceipt){$receipt.recovery_probe=Get-Content -LiteralPath $probeReceipt -Raw|ConvertFrom-Json}
 }
 if($recoveryTask){$recoveryTask.Stop(0);$folder.DeleteTask(($name+'-Recovery'),0)}
 if($task){$task.Stop(0);$folder.DeleteTask($name,0)}
 foreach($p in @($parent,$worker)){
  if($p){$p.Refresh();if(-not $p.HasExited){if($p.Path-ine $exe){throw 'Fixture cleanup identity changed'};$p.Kill();$null=$p.WaitForExit(5000)}}
 }
 $receipt.finished_utc=[DateTime]::UtcNow.ToString('o');Record
}
$receipt|ConvertTo-Json -Depth 5

if(-not $receipt.passed){throw 'Native scheduled recovery failed; inspect the retained result.'}
