$ErrorActionPreference='Stop'
$installer=Join-Path (Split-Path $PSScriptRoot -Parent) 'supervisor.ps1'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$data=Join-Path $repo ('.audit\packaging-tests\install '+[guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $data
$exe=Join-Path $data 'ghost-reforged-supervisor.exe'
[IO.File]::WriteAllText($exe,'fixture')
[IO.File]::WriteAllText((Join-Path $data 'profiles.json'),'{}')
$tasks=@{};$changes=New-Object 'Collections.Generic.List[string]'
function Get-ScheduledTask { param($TaskName,$ErrorAction) $tasks[$TaskName] }
function New-ScheduledTaskAction { param($Execute,$Argument,$WorkingDirectory) [pscustomobject]@{Execute=$Execute;Arguments=$Argument;WorkingDirectory=$WorkingDirectory} }
function New-ScheduledTaskPrincipal { param($UserId,$LogonType,$RunLevel) [pscustomobject]@{UserId=$UserId;LogonType=$LogonType;RunLevel=$RunLevel} }
function New-ScheduledTaskTrigger { param([switch]$AtLogOn,$User,[switch]$Once,$At,$RepetitionInterval) [pscustomobject]@{AtLogOn=[bool]$AtLogOn;Once=[bool]$Once;Interval=$RepetitionInterval} }
function New-ScheduledTaskSettingsSet {
 param($ExecutionTimeLimit,$MultipleInstances,[switch]$StartWhenAvailable,[switch]$AllowStartIfOnBatteries,[switch]$DontStopIfGoingOnBatteries)
 [pscustomobject]@{ExecutionTimeLimit=$ExecutionTimeLimit;MultipleInstances=$MultipleInstances;Priority=7}
}
function New-ScheduledTask { param($Action,$Principal,$Trigger,$Settings,$Description) [pscustomobject]@{Actions=@($Action);Principal=$Principal;Triggers=@($Trigger);Settings=$Settings;State='Ready'} }
function Register-ScheduledTask {
 param($TaskName,$InputObject,[switch]$Force)
 if($tasks.ContainsKey($TaskName)){$InputObject.State=$tasks[$TaskName].State}
 $tasks[$TaskName]=$InputObject;$changes.Add('register:'+ $TaskName)
}
function Unregister-ScheduledTask { param($TaskName,$Confirm) $tasks.Remove($TaskName);$changes.Add('remove:'+ $TaskName) }
function Start-ScheduledTask { param($TaskName) $changes.Add('start:'+ $TaskName) }
function Stop-ScheduledTask { throw 'Installation must not interrupt a running supervisor' }
$primary='GHostReforged-Supervisor';$guard='GHostReforged-Recovery'
& $installer -DataDirectory $data -Executable $exe -User 'fixture'|Out-Null
if($tasks.Count-ne 2-or $changes[0]-cne ('register:'+$guard)){throw 'Both tasks must be installed, recovery first'}
if($tasks[$primary].Settings.MultipleInstances-ne 'Parallel'-or $tasks[$guard].Settings.MultipleInstances-ne 'IgnoreNew'-or $tasks[$guard].Settings.ExecutionTimeLimit.TotalSeconds-ne 45){throw 'Incorrect task instance policies or unbounded recovery'}
if($tasks[$primary].Settings.Priority-ne 4-or $tasks[$guard].Settings.Priority-ne 4){throw 'Both the supervisor and its bounded recovery check need normal scheduler priority'}
if(-not $tasks[$primary].Triggers[0].AtLogOn-or $tasks[$guard].Triggers[0].Interval.TotalSeconds-ne 60){throw 'Incorrect recovery schedule'}
if($tasks[$guard].Actions[0].Arguments-notlike '*recover-supervisor.ps1" -DataDirectory "*" -Executable "*"'){throw 'Quoted recovery command is missing'}
$tasks[$primary].State='Running';$changes.Clear()
& $installer -DataDirectory $data -Executable $exe -User 'fixture'|Out-Null
if($changes.Count-ne 2-or $tasks[$primary].State-ne 'Running'){throw 'Live reinstall interrupted the task'}
$original=$tasks[$guard].Actions[0].Arguments
$tasks[$guard].Actions[0].Arguments='foreign';$changes.Clear();$rejected=$false
try{& $installer -DataDirectory $data -Executable $exe -User 'fixture'|Out-Null}catch{$rejected=$true}
if(-not $rejected-or $changes.Count){throw 'Foreign recovery task was changed'}
$tasks[$guard].Actions[0].Arguments=$original;$rejected=$false
try{& $installer -DataDirectory $data -Executable $exe -User 'fixture' -Remove|Out-Null}catch{$rejected=$true}
if(-not $rejected-or $changes.Count){throw 'Live removal must preserve both tasks'}
$tasks[$primary].State='Ready'
& $installer -DataDirectory $data -Executable $exe -User 'fixture' -Remove|Out-Null
if($tasks.Count-or $changes.Count-ne 2){throw 'Removal left a recovery task behind'}
$changes.Clear();$copy=Join-Path $data 'supervisor.ps1';Copy-Item -LiteralPath $installer -Destination $copy
$rejected=$false
try{& $copy -DataDirectory $data -Executable $exe -User 'fixture'|Out-Null}catch{$rejected=$true}
if(-not $rejected-or $changes.Count){throw 'Incomplete bundle mutated scheduled tasks'}
Write-Output 'PASS: paired install, periodic recovery, live reinstall, foreign task protection, paired removal, incomplete bundle.'
