param([Parameter(Mandatory)][string]$OutputFile)
$ErrorActionPreference = 'Stop'
$cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
$metadata = & $cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata could not be read.' }
$resolved = @{}
foreach ($node in $metadata.resolve.nodes) { $resolved[$node.id] = $true }
$output = New-Object Text.StringBuilder
[void]$output.AppendLine('GHost Reforged dependency notices')
foreach ($package in ($metadata.packages | Sort-Object name, version)) {
    if ($package.name -eq 'ghost-reforged' -or -not $resolved.ContainsKey($package.id)) { continue }
    $directory = Split-Path -Parent $package.manifest_path
    $licenses = @(Get-ChildItem -LiteralPath $directory -File |
        Where-Object { $_.Name -match '^(LICENSE|COPYING|COPYRIGHT|NOTICE)([._-]|$)' })
    if ($package.license_file) {
        $custom = Join-Path $directory $package.license_file
        if (Test-Path -LiteralPath $custom -PathType Leaf) { $licenses += Get-Item -LiteralPath $custom }
    }
    if (-not $licenses.Count) {
        $repository = $package.repository -replace '^https://github.com/', '' -replace '/tree/.*$', ''
        if ($package.name -eq 'zune-core') { $repository = 'etemesi254/zune-image' }
        $fallback = Join-Path (Join-Path $PSScriptRoot '..\licenses') ($repository -replace '/', '-')
        if ($repository -and (Test-Path -LiteralPath $fallback -PathType Container)) {
            $licenses = @(Get-ChildItem -LiteralPath $fallback -File)
        }
    }
    if (-not $licenses.Count) { throw "No license text for $($package.name) $($package.version)." }
    [void]$output.AppendLine("`n===== $($package.name) $($package.version) ($($package.license)) =====")
    foreach ($license in ($licenses | Sort-Object FullName -Unique)) {
        [void]$output.AppendLine("`n$($license.Name)`n")
        [void]$output.AppendLine((Get-Content -Raw -LiteralPath $license.FullName))
    }
}
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputFile), $output.ToString(), (New-Object Text.UTF8Encoding $false))
