param(
    [Parameter(Mandatory = $true)][string]$GameDirectory,
    [switch]$VerifySignatures
)
$ErrorActionPreference = 'Stop'
# Explicit inventory only: no account files, command lines or installation paths.
$directory = [IO.Path]::GetFullPath($GameDirectory)
if (-not (Test-Path -LiteralPath (Join-Path $directory 'Warcraft III.exe') -PathType Leaf)) {
    throw 'Select the x86_64 directory containing Warcraft III.exe.'
}
$required = @(
    'Warcraft III.exe',
    'BlizzardBrowser\BlizzardBrowser.exe',
    'BlizzardBrowser\libcef.dll',
    'BlizzardBrowser\swiftshader\libEGL.dll',
    'BlizzardBrowser\swiftshader\libGLESv2.dll'
)
$optional = @('d3d11.dll', 'dxgi.dll', 'd3d9.dll', 'opengl32.dll', 'd3dcompiler_47.dll')
$rows = @(foreach ($name in $required + $optional) {
    $path = Join-Path $directory $name
    $present = Test-Path -LiteralPath $path -PathType Leaf
    $row = [ordered]@{file=$name;required=($name -in $required);present=$present}
    if ($present) {
        $file = Get-Item -LiteralPath $path
        $row.bytes = $file.Length
        $row.version = $file.VersionInfo.FileVersion
        $row.sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        if ($VerifySignatures) {
            $row.signature = [string](Get-AuthenticodeSignature -LiteralPath $path).Status
        }
    }
    $row
})
[ordered]@{
    schema_version=1
    utc=[DateTime]::UtcNow.ToString('o')
    required_files_present=(@($rows | Where-Object { $_.required -and -not $_.present }).Count -eq 0)
    files=$rows
} | ConvertTo-Json -Depth 4
