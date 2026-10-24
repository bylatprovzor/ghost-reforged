$ErrorActionPreference = 'Stop'
$script = Join-Path (Split-Path $PSScriptRoot -Parent) 'inspect-runtime.ps1'
$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$parent = Join-Path $repository '.audit\runtime-tests'
$root = Join-Path $parent ([Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $root 'BlizzardBrowser\swiftshader') -Force | Out-Null
$required = @('Warcraft III.exe', 'BlizzardBrowser\BlizzardBrowser.exe', 'BlizzardBrowser\libcef.dll', 'BlizzardBrowser\swiftshader\libEGL.dll', 'BlizzardBrowser\swiftshader\libGLESv2.dll')
$signatureTracker = [pscustomobject]@{reads=0}
function Get-AuthenticodeSignature {
    param($LiteralPath)
    $signatureTracker.reads++
    [pscustomobject]@{Status='NotSigned'}
}
try {
    foreach ($name in $required) { [IO.File]::WriteAllText((Join-Path $root $name), $name) }
    [IO.File]::WriteAllText((Join-Path $root 'account.json'), 'must-never-appear')
    $before = (Get-FileHash -LiteralPath (Join-Path $root 'Warcraft III.exe')).Hash
    $raw = & $script -GameDirectory $root
    $report = $raw | ConvertFrom-Json
    if (-not $report.required_files_present -or $report.files.Count -ne 10) { throw 'Required inventory is incomplete' }
    if ($raw.Contains($root) -or $raw.Contains('account.json') -or $raw.Contains('must-never-appear')) { throw 'Inventory leaked unlisted data' }
    if ($signatureTracker.reads -ne 0) { throw 'Expensive signature checks ran without request' }
    if ($report.files[0].sha256 -ne $before -or (Get-FileHash -LiteralPath (Join-Path $root 'Warcraft III.exe')).Hash -ne $before) { throw 'Inventory changed files or reported the wrong digest' }
    [IO.File]::WriteAllText((Join-Path $root 'dxgi.dll'), 'replacement')
    $report = & $script -GameDirectory $root -VerifySignatures | ConvertFrom-Json
    $override = @($report.files | Where-Object file -eq 'dxgi.dll')
    if ($signatureTracker.reads -ne 6 -or $override.Count -ne 1 -or -not $override[0].present -or $override[0].required) { throw 'Optional runtime override was not identified' }
    Remove-Item -LiteralPath (Join-Path $root 'BlizzardBrowser\libcef.dll')
    $report = & $script -GameDirectory $root | ConvertFrom-Json
    if ($report.required_files_present) { throw 'Missing required library was reported as complete' }
    $rejected = $false
    try { & $script -GameDirectory (Join-Path $root 'missing') | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw 'Missing game executable was accepted' }
    'PASS: explicit inventory, file hashes, privacy, optional signatures, overrides and missing files.'
} finally {
    $resolved = (Resolve-Path -LiteralPath $root).Path
    if (-not $resolved.StartsWith($parent.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unexpected test cleanup directory' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
