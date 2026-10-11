param([Parameter(Mandatory)][string]$Archive, [Parameter(Mandatory)][string]$InstallDirectory, [switch]$NoShortcut)
$ErrorActionPreference = 'Stop'
try {
    $staging = Join-Path $PSScriptRoot 'payload'
    Expand-Archive -LiteralPath $Archive -DestinationPath $staging
    $roots = @(Get-ChildItem -LiteralPath $staging -Directory)
    if ($roots.Count -ne 1) { throw 'Invalid installation package.' }
    & (Join-Path $roots[0].FullName 'install.ps1') -InstallDirectory $InstallDirectory -NoShortcut:$NoShortcut
} catch {
    Write-Error $_
    exit 1
}
