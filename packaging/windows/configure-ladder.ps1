param(
    [Parameter(Mandatory = $true)][string]$ProtectedCredentialPath,
    [switch]$Apply
)
$ErrorActionPreference = 'Stop'
$path = [IO.Path]::GetFullPath($ProtectedCredentialPath)
$file = Get-Item -LiteralPath $path -ErrorAction Stop
if ($file.PSIsContainer -or $file.Length -lt 1 -or $file.Length -gt 8192) {
    throw 'A bounded Windows DPAPI credential file is required.'
}
Add-Type -AssemblyName System.Security
$plain = $null
try {
    $plain = [Security.Cryptography.ProtectedData]::Unprotect(
        [IO.File]::ReadAllBytes($path), $null,
        [Security.Cryptography.DataProtectionScope]::CurrentUser)
    if ([Text.Encoding]::UTF8.GetString($plain) -cnotmatch '^[A-Za-z0-9_-]{43,128}$') {
        throw 'The protected credential has an invalid format.'
    }
} catch {
    throw 'The protected ladder credential is unavailable to this Windows user.'
} finally {
    if ($null -ne $plain) { [Array]::Clear($plain, 0, $plain.Length) }
}
foreach ($scope in @('Process', 'User', 'Machine')) {
    if ($null -ne [Environment]::GetEnvironmentVariable('W3LADDER_COLLECTOR_KEY', $scope)) {
        throw 'An explicit collector key overrides the file. Resolve that override before applying.'
    }
}
if ($Apply) {
    [Environment]::SetEnvironmentVariable('W3LADDER_COLLECTOR_KEY_FILE', $path, 'User')
    Write-Output 'Protected ladder credential configured. Sign in again or reboot before hosting.'
} else {
    Write-Output 'Protected ladder credential is readable and valid. Use -Apply to persist its location.'
}
