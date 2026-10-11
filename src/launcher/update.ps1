param([int]$WaitPid, [string]$Application, [string]$DataDirectory,
      [string]$ExpectedHash, [switch]$ResumeHosting)
$ErrorActionPreference = 'Stop'
$setup = Join-Path $PSScriptRoot 'GHost-Reforged-Setup-x64.exe'
$parent = Get-Process -Id $WaitPid -ErrorAction SilentlyContinue
if ($parent -and -not $parent.WaitForExit(30000)) { exit 2 }
try {
    if ((Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash -ine $ExpectedHash) {
        throw 'The installer checksum changed.'
    }
    $destination = Split-Path -Parent $Application
    $install = Start-Process -FilePath $setup -ArgumentList @('--silent', '--no-launch', '--install-dir', ('"' + $destination + '"')) -WindowStyle Hidden -Wait -PassThru
    if ($install.ExitCode -ne 0) { throw 'Installation could not finish.' }
    $Application = Join-Path $destination 'ghost-reforged.exe'
} catch {
    $_.Exception.Message | Set-Content -LiteralPath (Join-Path $DataDirectory 'update-error.txt')
} finally {
    # Restart the previous version as well if installation failed.
    $arguments = @('--data-dir', ('"' + $DataDirectory + '"'))
    if ($ResumeHosting) { $arguments += '--host' }
    Start-Process -FilePath $Application -ArgumentList $arguments
    Remove-Item -LiteralPath $setup -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath (Join-Path $PSScriptRoot 'update.ps1') -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $PSScriptRoot -ErrorAction SilentlyContinue
}
