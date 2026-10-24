param(
    [Parameter(Mandatory = $true)][string]$DataDirectory,
    [Parameter(Mandatory = $true)][string]$Executable,
    [string]$TaskName='GHostReforged-Supervisor'
)
$ErrorActionPreference = 'Stop'
$exe = [IO.Path]::GetFullPath($Executable)
$data = [IO.Path]::GetFullPath($DataDirectory).TrimEnd('\')
if ($exe -match '["\r\n]' -or $data -match '["\r\n]') { throw 'Invalid path.' }
if ([IO.Path]::GetFileName($exe) -ine 'ghost-reforged-supervisor.exe' -or
    -not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Supervisor executable is missing.' }
if (-not (Test-Path -LiteralPath (Join-Path $data 'profiles.json') -PathType Leaf)) {
    throw 'Configured profile directory is required.'
}
function Read-Intent {
    $path = Join-Path $data 'hosting-intent.json'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $null }
    if ((Get-Item -LiteralPath $path).Length -gt 1024) { throw 'Invalid hosting intent.' }
    $intent = [IO.File]::ReadAllText($path) | ConvertFrom-Json
    if ($intent.version -ne 1 -or $intent.hosting -isnot [bool] -or
        ($null -ne $intent.quit -and $intent.quit -isnot [bool]) -or
        @($intent.PSObject.Properties.Name | Where-Object { $_ -notin @('version','hosting','quit') }).Count) {
        throw 'Invalid hosting intent.'
    }
    return $intent
}
$intent = Read-Intent
# Recovery never turns an intentional Stop or Quit into a hosting request.
if (-not $intent -or $intent.quit) { return }

# Use the supervisor's native lock, not a PID or the scheduler's running state.
# A parent can disappear without the scheduler restarting it; the controller
# may still be alive and must be adopted instead of being duplicated.
# Exclusive sharing fails immediately while the native supervisor retains this
# file. Do not enter a potentially blocking managed byte-range lock operation.
try {
    $lock = [IO.File]::Open((Join-Path $data 'supervisor.lock'), [IO.FileMode]::OpenOrCreate,
        [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
} catch [IO.IOException] {
    if (($_.Exception.GetBaseException().HResult -band 0xffff) -eq 32) { return }
    throw
}
try {
    $intent = Read-Intent
    if (-not $intent -or $intent.quit) { return }
} finally { $lock.Dispose() }

# Launch in the supervisor task's job, not as a child of this bounded check.
# Otherwise a recovery task's time limit can terminate the controller it started.
$schedule=New-Object -ComObject Schedule.Service
$schedule.Connect()
$task=$schedule.GetFolder('\').GetTask($TaskName)
$actions=@($task.Definition.Actions)
$principal=$task.Definition.Principal.UserId
$ownerSid=if ($principal -match '^S-1-') {
    ([Security.Principal.SecurityIdentifier]::new($principal)).Value
} else {
    ([Security.Principal.NTAccount]::new($principal)).Translate([Security.Principal.SecurityIdentifier]).Value
}
if ($actions.Count -ne 1 -or $actions[0].Path -ine $exe -or
    $actions[0].Arguments -cne ('--data-dir "'+$data+'"') -or
    $task.Definition.Principal.LogonType -ne 3 -or
    $ownerSid -ne [Security.Principal.WindowsIdentity]::GetCurrent().User.Value) {
    throw 'Recovery task does not identify this interactive supervisor.'
}
$intent=Read-Intent
if ($intent -and -not $intent.quit) { $null=$task.Run($null) }
