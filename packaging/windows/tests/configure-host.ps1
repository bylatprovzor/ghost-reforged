$ErrorActionPreference = 'Stop'
$script = Join-Path (Split-Path $PSScriptRoot -Parent) 'configure-host.ps1'
$root = Join-Path ([IO.Path]::GetTempPath()) ('ghost-preferences-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$path = Join-Path $root 'War3Preferences.txt'
$fixture = @'
[Misc]
seenintromovie=0
untouched=73
[Video]
windowwidth=1920
windowheight=1080
reswidth=1920
resheight=1080
windowmode=1
vsync=1
maxfps=144
backgroundmaxfps=30
lightingquality=2
particles=2
pointlightshadowquality=2
shadowquality=2
texquality=2
spellfilter=2
[Sound]
music=1
sfx=1
musicvolume=80
sfxvolume=80
'@
# Only the temporary preferences file is exercised; never inspect or alter games.
function Get-Process { param($Name, $ErrorAction) [pscustomobject]@{Id=1} }
try {
    [IO.File]::WriteAllText($path, $fixture)
    $before = (Get-FileHash -LiteralPath $path).Hash
    $preview = & $script -Preferences $path | ConvertFrom-Json
    if ($preview.applied -or (Get-FileHash -LiteralPath $path).Hash -ne $before) { throw 'Preview changed preferences' }
    $rejected = $false
    try { & $script -Preferences $path -Apply | Out-Null } catch { $rejected = $_.Exception.Message -like 'Close Warcraft*' }
    if (-not $rejected -or (Get-FileHash -LiteralPath $path).Hash -ne $before) { throw 'Live-game guard failed' }
    function Get-Process { param($Name, $ErrorAction) }
    $applied = & $script -Preferences $path -Apply | ConvertFrom-Json
    $after = (Get-FileHash -LiteralPath $path).Hash
    if (-not $applied.changed -or -not ([IO.File]::ReadAllText($path)).Contains('untouched=73')) { throw 'Unrelated preferences were lost' }
    $backups = @(Get-ChildItem -LiteralPath $root -Filter '*.backup')
    if ($backups.Count -ne 1 -or (Get-FileHash -LiteralPath $backups[0].FullName).Hash -ne $before) { throw 'Original backup is not exact' }
    $again = & $script -Preferences $path -Apply | ConvertFrom-Json
    if ($again.changed -or (Get-FileHash -LiteralPath $path).Hash -ne $after -or @(Get-ChildItem -LiteralPath $root -Filter '*.backup').Count -ne 1) { throw 'Preset is not idempotent' }
    foreach ($invalid in @($fixture.Replace('maxfps=144', 'unknownfps=144'), $fixture.Replace('maxfps=144', "maxfps=144`nmaxfps=90"))) {
        [IO.File]::WriteAllText($path, $invalid)
        $before = (Get-FileHash -LiteralPath $path).Hash
        $rejected = $false
        try { & $script -Preferences $path -Apply | Out-Null } catch { $rejected = $true }
        if (-not $rejected -or (Get-FileHash -LiteralPath $path).Hash -ne $before) { throw 'Malformed preferences were partially overwritten' }
    }
    Write-Output 'PASS: preview, live-game guard, exact backup, unrelated settings, idempotence, missing and duplicate keys.'
} finally {
    $resolved = (Resolve-Path -LiteralPath $root).Path
    $parent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($parent, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -notlike 'ghost-preferences-*') { throw 'Unexpected test cleanup directory' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
