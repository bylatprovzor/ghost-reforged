$ErrorActionPreference='Stop'
$installer=Join-Path (Split-Path $PSScriptRoot -Parent) 'install-maintenance.ps1'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$data=Join-Path $repo ('.audit\packaging-tests\maintenance '+[guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $data
foreach($name in @('profiles.json','python.exe','pythonw.exe')) { [IO.File]::WriteAllText((Join-Path $data $name),'fixture') }
$tasks=@();$registrations=New-Object 'Collections.Generic.List[object]'
$folder=[pscustomobject]@{}
$folder|Add-Member ScriptMethod GetTasks { param($flags) if($flags-ne 1){throw 'Hidden tasks must be checked'}; return $tasks }
$folder|Add-Member ScriptMethod RegisterTaskDefinition {param($name,$task,$flags,$user,$password,$logon) $registrations.Add($task)}
$service=[pscustomobject]@{}
$service|Add-Member ScriptMethod Connect {}
$service|Add-Member ScriptMethod GetFolder {param($path) return $folder}
$service|Add-Member ScriptMethod NewTask {
 param($flags)
 $triggers=[pscustomobject]@{}
 $triggers|Add-Member ScriptMethod Create {param($type) return [pscustomobject]@{StartBoundary='';Repetition=[pscustomobject]@{Interval=''};Enabled=$false}}
 $actions=[pscustomobject]@{Item=$null}
 $actions|Add-Member ScriptMethod Create {param($type) $this.Item=[pscustomobject]@{Path='';Arguments='';WorkingDirectory=''};return $this.Item}
 return [pscustomobject]@{
  RegistrationInfo=[pscustomobject]@{Description=''}
  Principal=[pscustomobject]@{UserId='';LogonType=0;RunLevel=0}
  Settings=[pscustomobject]@{Enabled=$false;Hidden=$false;StartWhenAvailable=$false;DisallowStartIfOnBatteries=$true;StopIfGoingOnBatteries=$true;MultipleInstances=0;ExecutionTimeLimit='';Priority=7}
  Triggers=$triggers;Actions=$actions
 }
}
function New-Object {param($ComObject) if($ComObject-ne 'Schedule.Service'){throw 'Unexpected COM request'};return $service}
$python=Join-Path $data 'python.exe'
& $installer -DataDirectory ($data+'\') -Python $python|Out-Null
if($registrations.Count-ne 1){throw 'Maintenance not installed'}
$task=$registrations[0]
$expected='"'+(Join-Path (Split-Path $PSScriptRoot -Parent) 'maintain-runtime.py')+'" --state-dir "'+$data+'" --apply'
if($task.Actions.Item.Arguments-cne $expected){throw 'Trailing separator changed the quoted state path'}
if($task.Settings.MultipleInstances-ne 2-or $task.Settings.ExecutionTimeLimit-cne 'PT5M'){throw 'Maintenance is not bounded'}
if($task.Settings.Priority-ne 4){throw 'Maintenance can be starved by the game at the default scheduler priority'}
$xml='<Task><Actions><Exec><Command>foreign.exe</Command><Arguments>foreign</Arguments></Exec></Actions></Task>'
$tasks=@([pscustomobject]@{Name='ghostreforged-maintenance';Xml=$xml})
$rejected=$false
try { & $installer -DataDirectory $data -Python $python|Out-Null } catch { $rejected=$true }
if(-not $rejected-or $registrations.Count-ne 1){throw 'Hidden foreign task was overwritten'}
$rejected=$false
try { & $installer -DataDirectory 'C:\' -Python $python|Out-Null } catch { $rejected=$true }
if(-not $rejected-or $registrations.Count-ne 1){throw 'Drive root was accepted'}
Write-Output 'PASS: trailing-separator quoting, hidden/case-insensitive task collision, bounded execution, normal scheduler priority, drive-root rejection.'
