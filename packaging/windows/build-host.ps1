$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
$rustc = (Get-Command rustc.exe -ErrorAction Stop).Source
$git = (Get-Command git.exe -ErrorAction Stop).Source
$build = Join-Path $root 'target\vps'
$packageRoot = Join-Path $root 'target\host-package'
$version = [regex]::Match([IO.File]::ReadAllText((Join-Path $root 'Cargo.toml')), '(?m)^version = "([0-9.]+)"').Groups[1].Value
if (-not $version) { throw 'Package version is unavailable.' }
$package = Join-Path $packageRoot ('GHost-Reforged-Host-' + $version + '-' + [Guid]::NewGuid().ToString('N'))
Push-Location $root
$previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
try {
    $sysroot = (& $rustc --print sysroot).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain is unavailable.' }
    $env:CARGO_ENCODED_RUSTFLAGS = @('--remap-path-prefix', "$env:USERPROFILE=/builder", '--remap-path-prefix', "$sysroot=/rust", '--remap-path-prefix', "$root=/source/ghost-reforged", '-C', 'debuginfo=0', '-C', 'target-feature=+crt-static') -join [char]31
    & $cargo build --locked --release --no-default-features --bin ghost-reforged-supervisor --bin ghost-reforged-cli --target-dir $build
    if ($LASTEXITCODE -ne 0) { throw 'Host build failed.' }
    New-Item -ItemType Directory -Path $package -Force | Out-Null
    foreach ($name in @('ghost-reforged-supervisor.exe', 'ghost-reforged-cli.exe')) {
        Copy-Item -LiteralPath (Join-Path $build ('release\' + $name)) -Destination $package
    }
    foreach ($name in @('supervisor.ps1', 'recover-supervisor.ps1', 'diagnose.ps1', 'inspect-runtime.ps1', 'configure-host.ps1', 'configure-ladder.ps1', 'maintain-runtime.py', 'install-maintenance.ps1', 'VPS.md')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $package
    }
    foreach ($name in @('LICENSE', 'NOTICE')) {
        Copy-Item -LiteralPath (Join-Path $root $name) -Destination $package
    }
    & (Join-Path $PSScriptRoot 'licenses.ps1') -OutputFile (Join-Path $package 'THIRD-PARTY-LICENSES.txt')
    if (-not $?) { throw 'Dependency licenses could not be collected.' }
    $hashes = [ordered]@{}
    Get-ChildItem -LiteralPath $package -File | Sort-Object Name | ForEach-Object {
        $hashes[$_.Name] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }
    $revision = (& $git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Source revision is unavailable.' }
    $dirty = @(& $git status --porcelain --untracked-files=normal).Count -gt 0
    if ($LASTEXITCODE -ne 0) { throw 'Source state is unavailable.' }
    [ordered]@{version=$version;source_revision=$revision;source_dirty=$dirty;files=$hashes} |
        ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $package 'manifest.json') -Encoding UTF8
    $archive = Join-Path $packageRoot "GHost-Reforged-Host-$version-windows-x64.zip"
    Compress-Archive -LiteralPath $package -DestinationPath $archive -Force
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
    "$hash  $([IO.Path]::GetFileName($archive))" | Set-Content -LiteralPath "$archive.sha256" -Encoding ASCII
    [ordered]@{archive=$archive;sha256=$hash;bytes=(Get-Item -LiteralPath $archive).Length;source_dirty=$dirty} | ConvertTo-Json
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags
    Pop-Location
}
