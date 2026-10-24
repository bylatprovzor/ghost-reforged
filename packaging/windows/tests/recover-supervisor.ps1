$ErrorActionPreference='Stop'
$script=Join-Path (Split-Path $PSScriptRoot -Parent) 'recover-supervisor.ps1'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$directory=Join-Path $repo ('.audit\packaging-tests\recovery-'+[guid]::NewGuid().ToString('N'))
$null=New-Item -ItemType Directory -Path $directory
$exe=Join-Path $directory 'ghost-reforged-supervisor.exe'
$intent=Join-Path $directory 'hosting-intent.json'
[IO.File]::WriteAllText($exe,'fixture')
[IO.File]::WriteAllText((Join-Path $directory 'profiles.json'),'{}')
$calls=New-Object 'Collections.Generic.List[object]'
$nativeUser=[Security.Principal.WindowsIdentity]::GetCurrent().Name
$task=[pscustomobject]@{Definition=[pscustomobject]@{Actions=@([pscustomobject]@{Path=$exe;Arguments='--data-dir "'+$directory+'"'});Principal=[pscustomobject]@{LogonType=3;UserId=$nativeUser}}}
$task|Add-Member -MemberType ScriptMethod -Name Run -Value {param($unused) $calls.Add('run')}
$folder=[pscustomobject]@{}
$folder|Add-Member -MemberType ScriptMethod -Name GetTask -Value {param($name) if($name-cne 'GHostReforged-Supervisor'){throw 'Unexpected task'};return $task}
$service=[pscustomobject]@{}
$service|Add-Member -MemberType ScriptMethod -Name Connect -Value {}
$service|Add-Member -MemberType ScriptMethod -Name GetFolder -Value {param($name) return $folder}
function New-Object {param($ComObject) if($ComObject-ne 'Schedule.Service'){throw 'Unexpected COM request'};return $service}
function Start-Process {throw 'Recovery must never own a supervisor child'}
& $script -DataDirectory $directory -Executable $exe
if($calls.Count-ne 0){throw 'Missing intent triggered recovery'}
foreach($json in @('{"version":1,"hosting":false,"quit":true}','{"version":1,"hosting":true,"quit":true}')){
 [IO.File]::WriteAllText($intent,$json)
 & $script -DataDirectory $directory -Executable $exe
 if($calls.Count-ne 0){throw 'Quit was ignored'}
}
foreach($json in @('{"version":1,"hosting":false,"quit":false}','{"version":1,"hosting":true,"quit":false}')){
 [IO.File]::WriteAllText($intent,$json);$before=$calls.Count
 & $script -DataDirectory $directory -Executable $exe
 if($calls.Count-ne $before+1-or [IO.File]::ReadAllText($intent)-cne $json){throw 'Saved hosting intent changed'}
}
$before=$calls.Count
$lock=[IO.File]::Open((Join-Path $directory 'supervisor.lock'),[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::ReadWrite)
try{
 $lock.Lock(0,1)
 & $script -DataDirectory $directory -Executable $exe
 if($calls.Count-ne $before){throw 'A live supervisor was duplicated'}
}finally{$lock.Dispose()}
foreach($json in @('{','{"version":2,"hosting":true}','{"version":1,"hosting":"true"}','{"version":1,"hosting":true,"unknown":1}',('x'*1025))){
 [IO.File]::WriteAllText($intent,$json);$rejected=$false
 try{& $script -DataDirectory $directory -Executable $exe}catch{$rejected=$true}
 if(-not $rejected-or $calls.Count-ne $before){throw 'Invalid intent authorized recovery'}
}
Write-Output 'PASS: missing intent, Quit, stopped/active intent preservation, native file-lock exclusion, malformed intent.'
[IO.File]::WriteAllText($intent,'{"version":1,"hosting":true,"quit":false}')
$task.Definition.Actions[0].Path='foreign.exe';$before=$calls.Count;$rejected=$false
try {& $script -DataDirectory $directory -Executable $exe} catch {$rejected=$true}
if(-not $rejected-or $calls.Count-ne $before){throw 'Foreign supervisor task was started'}
Write-Output 'PASS: separate scheduler job and foreign task protection.'
$task.Definition.Actions[0].Path=$exe
$task.Definition.Principal.UserId=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$before=$calls.Count
& $script -DataDirectory $directory -Executable $exe
if($calls.Count-ne $before+1){throw 'A valid scheduler SID was rejected'}
Write-Output 'PASS: scheduler principals accept the same user as account name or SID.'
