param(
    [Parameter(Mandatory = $true)][string]$DataDirectory,
    [string]$Executable = '',
    [string]$User = [Security.Principal.WindowsIdentity]::GetCurrent().Name,
    [switch]$Start,
    [switch]$Remove,
    # Several hosts on one server: one name per Windows user (for example bot-1).
    [string]$Name = ''
)
$ErrorActionPreference = 'Stop'
# Windows PowerShell can leave $PSScriptRoot empty in parameter defaults.
$here = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $Executable) { $Executable = Join-Path $here 'ghost-reforged-supervisor.exe' }
if ($Name -and $Name -notmatch '^[A-Za-z0-9_-]{1,32}$') { throw 'Name must be 1-32 letters, digits, - or _.' }
$suffix = if ($Name) { '-' + $Name } else { '' }
$taskName = 'GHostReforged-Supervisor' + $suffix
$recoveryName = 'GHostReforged-Recovery' + $suffix
$recoveryScript = Join-Path $here 'recover-supervisor.ps1'
$powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$exe = [IO.Path]::GetFullPath($Executable)
$data = [IO.Path]::GetFullPath($DataDirectory).TrimEnd('\')
if ($exe -match '["\r\n]' -or $data -match '["\r\n]') { throw 'Invalid path.' }
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Supervisor executable is missing.' }
if ([IO.Path]::GetFileName($exe) -ine 'ghost-reforged-supervisor.exe') { throw 'Unexpected supervisor executable.' }
if (-not (Test-Path -LiteralPath (Join-Path $data 'profiles.json') -PathType Leaf)) {
    throw 'Configure the hosting profile before installing the supervisor.'
}
$arguments = '--data-dir "' + $data + '"'
$recoveryArguments = '-NoProfile -NonInteractive -WindowStyle Hidden -File "' + $recoveryScript +
    '" -DataDirectory "' + $data + '" -Executable "' + $exe + '"'
if ($Name) { $recoveryArguments += ' -TaskName "' + $taskName + '"' }
$existing = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
$recovery = Get-ScheduledTask -TaskName $recoveryName -ErrorAction SilentlyContinue
if ($existing) {
    $actions = @($existing.Actions)
    if ($actions.Count -ne 1 -or $actions[0].Execute -ine $exe -or $actions[0].Arguments -cne $arguments) {
        throw 'A different supervisor task already exists. It has not been changed.'
    }
}
if ($recovery) {
    $actions = @($recovery.Actions)
    if ($actions.Count -ne 1 -or $actions[0].Execute -ine $powershell -or $actions[0].Arguments -cne $recoveryArguments) {
        throw 'A different recovery task already exists. It has not been changed.'
    }
}
if ($Remove) {
    if (($existing -and $existing.State -eq 'Running') -or ($recovery -and $recovery.State -eq 'Running')) {
        throw 'Quit the controller before removing its supervisor tasks.'
    }
    if ($recovery) { Unregister-ScheduledTask -TaskName $recoveryName -Confirm:$false }
    if ($existing) {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
    }
    Write-Output 'Supervisor startup removed.'
    return
}
if (-not (Test-Path -LiteralPath $recoveryScript -PathType Leaf)) { throw 'Recovery script is missing.' }
$action = New-ScheduledTaskAction -Execute $exe -Argument $arguments -WorkingDirectory ([IO.Path]::GetDirectoryName($exe))
$principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Limited
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $User
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances Parallel `
    -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$settings.Priority = 4
$task = New-ScheduledTask -Action $action -Principal $principal -Trigger $trigger -Settings $settings `
    -Description 'GHost Reforged resident supervisor. Requires a logged-on Windows session; RDP may disconnect.'
$recoveryAction = New-ScheduledTaskAction -Execute $powershell -Argument $recoveryArguments -WorkingDirectory $here
$recoveryTrigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1)
# Checks never own controller children. Bound their lifetime and concurrency;
# the primary task permits adoption even when an orphan retains its old job.
# Its native lock still excludes a second resident supervisor.
$recoverySettings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds 45) -MultipleInstances IgnoreNew `
    -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
# A check starved by the game can expire before it observes the native lock.
$recoverySettings.Priority = 4
$recoveryTask = New-ScheduledTask -Action $recoveryAction -Principal $principal -Trigger $recoveryTrigger -Settings $recoverySettings `
    -Description 'Check the GHost supervisor every minute; preserve saved Stop and Quit intent.'
Register-ScheduledTask -TaskName $recoveryName -InputObject $recoveryTask -Force | Out-Null
Register-ScheduledTask -TaskName $taskName -InputObject $task -Force | Out-Null
if ($Start) { Start-ScheduledTask -TaskName $taskName }
Write-Output 'Supervisor and recurring recovery installed. Saved Stop and Quit intent are preserved.'
