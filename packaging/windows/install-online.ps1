param([string]$Version = 'v0.2.3', [string]$InstallDirectory, [switch]$NoShortcut)
& {
    param($ReleaseVersion, $Destination, $SkipShortcut)
    $ErrorActionPreference = 'Stop'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $repository = 'shideneyu/ghost-reforged'
    if ($ReleaseVersion -eq 'latest') {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$repository/releases/latest"
        $ReleaseVersion = $release.tag_name
    }
    if ($ReleaseVersion -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') { throw 'Invalid release version.' }
    $base = "https://github.com/$repository/releases/download/$ReleaseVersion"
    $name = 'GHost-Reforged-Setup-x64.exe'
    $staging = Join-Path ([IO.Path]::GetTempPath()) ('ghost-download-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $staging | Out-Null
    try {
        $checksumsFile = Join-Path $staging 'SHA256SUMS'
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $checksumsFile
        $checksums = Get-Content -Raw -LiteralPath $checksumsFile
        $checksumMatches = @([regex]::Matches($checksums, '(?m)^([a-fA-F0-9]{64})  GHost-Reforged-Setup-x64\.exe\r?$'))
        if ($checksumMatches.Count -ne 1) { throw 'The release checksum is missing or ambiguous.' }
        $installer = Join-Path $staging $name
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $installer
        if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash -ine $checksumMatches[0].Groups[1].Value) {
            throw 'The installer checksum does not match. Nothing was installed.'
        }
        $arguments = @('--silent')
        if ($SkipShortcut) { $arguments += '--no-shortcut' }
        if ($Destination) {
            $resolved = [IO.Path]::GetFullPath($Destination)
            if ($resolved -match '["\r\n]') { throw 'Invalid installation directory.' }
            # Windows doubles trailing backslashes before a closing argument quote.
            $quoted = '"' + ($resolved -replace '(\\+)$', '$1$1') + '"'
            $arguments += @('--install-dir', $quoted)
        }
        $process = Start-Process -FilePath $installer -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw 'Installation failed. Close GHost Reforged and try again.' }
        Write-Host 'Installed. Open GHost Reforged from the Start menu.'
    } finally {
        $resolved = [IO.Path]::GetFullPath($staging)
        $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and
            [IO.Path]::GetFileName($resolved) -match '^ghost-download-[a-f0-9]{32}$') {
            Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
} $Version $InstallDirectory $NoShortcut
