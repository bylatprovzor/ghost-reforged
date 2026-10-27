param(
    # This host's name in the relay, for example bot-1. One per Windows user.
    [string]$Id = '',
    [string]$RelayDirectory = (Join-Path $env:ProgramData 'GHostRelay'),
    # Run once as an administrator: creates the shared directory for all hosts.
    [switch]$Prepare,
    # Leave the relay: this user hosts on its own again.
    [switch]$Remove
)
$ErrorActionPreference = 'Stop'
$dir = [IO.Path]::GetFullPath($RelayDirectory).TrimEnd('\')
if ($dir -match '["\r\n]') { throw 'Invalid path.' }
if ($Prepare) {
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $admin) { throw 'Run -Prepare from an administrator PowerShell.' }
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    # Every host writes only its own state file; all hosts read the others.
    & icacls $dir /grant '*S-1-5-32-545:(OI)(CI)M' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot grant the hosts access to the relay directory.' }
    Write-Output "Relay directory ready: $dir"
    return
}
if ($Remove) {
    [Environment]::SetEnvironmentVariable('GHOST_RELAY_ID', $null, 'User')
    [Environment]::SetEnvironmentVariable('GHOST_RELAY_DIR', $null, 'User')
    Write-Output 'This user left the relay. Restart hosting to apply.'
    return
}
if ($Id -notmatch '^[A-Za-z0-9_-]{1,32}$') { throw 'Id must be 1-32 letters, digits, - or _ (for example bot-1).' }
if (-not (Test-Path -LiteralPath $dir -PathType Container)) { throw 'Run relay-host.ps1 -Prepare as an administrator first.' }
$probe = Join-Path $dir ('.probe-' + $Id)
[IO.File]::WriteAllText($probe, 'ok'); Remove-Item -LiteralPath $probe
[Environment]::SetEnvironmentVariable('GHOST_RELAY_DIR', $dir, 'User')
[Environment]::SetEnvironmentVariable('GHOST_RELAY_ID', $Id, 'User')
Write-Output "This Windows user is relay host $Id. Sign out and back in (or restart the supervisor) to apply."
