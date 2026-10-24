param([string]$InstallDirectory, [switch]$NoShortcut)
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($InstallDirectory)) {
    if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is unavailable.' }
    $InstallDirectory = Join-Path $env:LOCALAPPDATA 'Programs\GHost Reforged'
}
$destination = [IO.Path]::GetFullPath($InstallDirectory)
$executable = Join-Path $destination 'ghost-reforged.exe'
$files = @('ghost-reforged.exe', 'ghost-reforged-cli.exe', 'ghost-reforged-supervisor.exe', 'supervisor.ps1', 'diagnose.ps1', 'configure-host.ps1', 'VPS.md', 'README.md', 'LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.txt')
foreach ($file in $files) {
    if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $file) -PathType Leaf)) {
        throw 'Extract the complete Windows ZIP before installing.'
    }
}
$programsToReplace = @($executable, (Join-Path $destination 'ghost-reforged-cli.exe'), (Join-Path $destination 'ghost-reforged-supervisor.exe'))
if (@(Get-Process -Name 'ghost-reforged','ghost-reforged-cli','ghost-reforged-supervisor' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and [IO.Path]::GetFullPath($_.Path) -iin $programsToReplace }).Count) {
    throw 'Close GHost Reforged before updating it.'
}
New-Item -ItemType Directory -Force -Path $destination | Out-Null
foreach ($file in $files) {
    $source = Join-Path $PSScriptRoot $file
    $target = Join-Path $destination $file
    if ([IO.Path]::GetFullPath($source) -ine [IO.Path]::GetFullPath($target)) {
        Copy-Item -LiteralPath $source -Destination $target -Force
    }
}
if (-not $NoShortcut) {
    $programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
    if (-not $programs) { throw 'The Start menu is unavailable.' }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut((Join-Path $programs 'GHost Reforged.lnk'))
    $shortcut.TargetPath = $executable
    $shortcut.WorkingDirectory = $destination
    $shortcut.IconLocation = "$executable,0"
    $shortcut.Description = 'Warcraft III auto-host'
    $shortcut.Save()
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) | Out-Null
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) | Out-Null
}
Write-Host 'Installed. Open GHost Reforged from the Start menu.'
