param(
    [string]$Preferences = (Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'Warcraft III\War3Preferences.txt'),
    [switch]$Apply
)
$ErrorActionPreference = 'Stop'
# A dedicated hosting preset. Preserve unrelated preferences and retain a backup.
$path = (Resolve-Path -LiteralPath $Preferences -ErrorAction Stop).Path
$settings = [ordered]@{
    'Misc.seenintromovie' = 1
    'Video.windowwidth' = 1024; 'Video.windowheight' = 768
    'Video.reswidth' = 1024; 'Video.resheight' = 768
    'Video.windowmode' = 0; 'Video.vsync' = 0
    'Video.maxfps' = 20; 'Video.backgroundmaxfps' = 5
    'Video.lightingquality' = 0; 'Video.particles' = 0
    'Video.pointlightshadowquality' = 0; 'Video.shadowquality' = 0
    'Video.texquality' = 0; 'Video.spellfilter' = 0
    'Sound.music' = 0; 'Sound.sfx' = 0
    'Sound.musicvolume' = 0; 'Sound.sfxvolume' = 0
}
$original = [IO.File]::ReadAllText($path)
$seen = @{}
$changes = @()
$section = ''
$newline = if ($original.Contains("`r`n")) { "`r`n" } else { "`n" }
$lines = foreach ($line in ($original -split '\r?\n')) {
    if ($line -match '^\s*\[([A-Za-z0-9]+)\]\s*$') {
        $section = $Matches[1]
        $line
    } elseif ($line -match '^\s*([A-Za-z0-9_]+)\s*=\s*(-?\d+)\s*$') {
        $key = $Matches[1]
        $current = [int]$Matches[2]
        $qualified = $section + '.' + $key
        if ($settings.Contains($qualified)) {
            if ($seen.ContainsKey($qualified)) { throw 'Duplicate hosting preference; nothing was changed.' }
            $seen[$qualified] = $true
            if ($current -ne $settings[$qualified]) {
                $changes += $qualified
                $key + '=' + $settings[$qualified]
            } else { $line }
        } else { $line }
    } else { $line }
}
if ($seen.Count -ne $settings.Count) {
    throw 'Expected hosting preferences are missing. Start and close the supported game once; nothing was changed.'
}
$backupSaved = $false
if ($Apply -and $changes.Count) {
    if (@(Get-Process -Name 'Warcraft III' -ErrorAction SilentlyContinue).Count) {
        throw 'Close Warcraft III before applying its hosting preferences.'
    }
    if ([IO.File]::ReadAllText($path) -cne $original) { throw 'Preferences changed during inspection; retry after closing the game.' }
    $suffix = [Guid]::NewGuid().ToString('N')
    $temporary = $path + '.' + $suffix + '.new'
    $backup = $path + '.' + $suffix + '.backup'
    try {
        [IO.File]::WriteAllText($temporary, ($lines -join $newline), (New-Object Text.UTF8Encoding($false)))
        [IO.File]::Replace($temporary, $path, $backup)
        $backupSaved = $true
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
    }
}
[ordered]@{
    applied = [bool]$Apply
    changed = ($Apply -and $changes.Count -gt 0)
    pending_or_applied_fields = $changes
    backup_saved = $backupSaved
    preset = $settings
} | ConvertTo-Json -Depth 3
