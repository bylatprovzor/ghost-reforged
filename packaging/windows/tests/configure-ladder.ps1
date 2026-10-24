$ErrorActionPreference = 'Stop'
$script = Join-Path (Split-Path $PSScriptRoot -Parent) 'configure-ladder.ps1'
$root = Join-Path ([IO.Path]::GetTempPath()) ('ghost-credential-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$path = Join-Path $root 'key.dpapi'
$before = [Environment]::GetEnvironmentVariable('W3LADDER_COLLECTOR_KEY_FILE', 'User')
Add-Type -AssemblyName System.Security
try {
    $key = [Text.Encoding]::UTF8.GetBytes('Synthetic_Packaging_Test_Credential_0123456789abcdef')
    $encrypted = [Security.Cryptography.ProtectedData]::Protect($key, $null, [Security.Cryptography.DataProtectionScope]::CurrentUser)
    [IO.File]::WriteAllBytes($path, $encrypted)
    $hash = (Get-FileHash -LiteralPath $path).Hash
    $preview = & $script -ProtectedCredentialPath $path
    if ($preview -notlike 'Protected ladder credential is readable*') { throw 'Valid protected credential was not recognized' }
    if ((Get-FileHash -LiteralPath $path).Hash -ne $hash) { throw 'Preview changed the credential' }
    foreach ($invalid in @([byte[]](1, 2, 3), (New-Object byte[] 8193))) {
        [IO.File]::WriteAllBytes($path, $invalid)
        $rejected = $false
        try { & $script -ProtectedCredentialPath $path -Apply | Out-Null } catch { $rejected = $true }
        if (-not $rejected) { throw 'Invalid protected credential was accepted' }
    }
    if ([Environment]::GetEnvironmentVariable('W3LADDER_COLLECTOR_KEY_FILE', 'User') -cne $before) { throw 'Failed validation changed the persisted environment' }
    Write-Output 'PASS: protected preview, unchanged file, corrupted and oversized input, persistent state preserved.'
} finally {
    $resolved = (Resolve-Path -LiteralPath $root).Path
    $parent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($parent, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -notlike 'ghost-credential-*') { throw 'Unexpected test cleanup directory' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
