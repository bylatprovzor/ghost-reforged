param([switch]$NoBuild, [switch]$Setup)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
$rustc = (Get-Command rustc.exe -ErrorAction Stop).Source
$target = 'x86_64-pc-windows-msvc'
Push-Location $root
try {
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $root 'Cargo.toml')
    $version = [regex]::Match($manifest, '(?m)^version = "([0-9.]+)"').Groups[1].Value
    if (-not $version) { throw 'Package version is unavailable.' }
    $buildRoot = Join-Path $root 'target'
    $package = Join-Path $buildRoot "package\GHost-Reforged-$version-windows-x64"
    if (Test-Path -LiteralPath $package) {
        $resolved = (Resolve-Path -LiteralPath $package).Path
        if (-not $resolved.StartsWith($buildRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Package directory is outside the build directory.'
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
    if (-not $NoBuild) {
        $previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
        try {
            $sysroot = (& $rustc --print sysroot).Trim()
            if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain is unavailable.' }
            $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
            $flags = @('--remap-path-prefix', "$env:USERPROFILE=/builder",
                '--remap-path-prefix', "$cargoHome=/cargo", '--remap-path-prefix', "$sysroot=/rust",
                '--remap-path-prefix', "$root=/source/ghost-reforged", '-C', 'debuginfo=0',
                '-C', 'target-feature=+crt-static', '-C', 'link-arg=/PDBALTPATH:ghost-reforged.pdb')
            $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]31
            & $cargo build --locked --release --target $target --target-dir $buildRoot --bin ghost-reforged
            if ($LASTEXITCODE -ne 0) { throw 'Windows build failed.' }
        } finally { $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags }
    }
    New-Item -ItemType Directory -Force -Path $package | Out-Null
    Copy-Item -LiteralPath (Join-Path $buildRoot "$target\release\ghost-reforged.exe") -Destination $package
    foreach ($file in @('README.md', 'LICENSE', 'NOTICE')) {
        Copy-Item -LiteralPath (Join-Path $root $file) -Destination $package
    }
    foreach ($file in @('Install.cmd', 'install.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $package
    }
    & (Join-Path $PSScriptRoot 'licenses.ps1') -OutputFile (Join-Path $package 'THIRD-PARTY-LICENSES.txt')
    if (-not $?) { throw 'Dependency licenses could not be collected.' }
    $zip = "$package.zip"
    Compress-Archive -LiteralPath $package -DestinationPath $zip -Force
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($zip))" | Set-Content -LiteralPath "$zip.sha256" -Encoding ascii
    Write-Host "Built $([IO.Path]::GetFileName($zip))"
    $release = Join-Path $buildRoot "release-assets\$version"
    New-Item -ItemType Directory -Force -Path $release | Out-Null
    Copy-Item -LiteralPath (Join-Path $package 'ghost-reforged.exe') -Destination $release -Force
    Copy-Item -LiteralPath $zip -Destination $release -Force
    foreach ($file in @('LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.txt')) {
        Copy-Item -LiteralPath (Join-Path $package $file) -Destination $release -Force
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install-online.ps1') -Destination (Join-Path $release 'install.ps1') -Force
    if ($Setup) {
        $previousFlags = $env:CARGO_ENCODED_RUSTFLAGS
        try {
            $sysroot = (& $rustc --print sysroot).Trim()
            if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain is unavailable.' }
            $flags = @('--remap-path-prefix', "$env:USERPROFILE=/builder", '--remap-path-prefix', "$sysroot=/rust",
                '--remap-path-prefix', "$root=/source/ghost-reforged", '-C', 'debuginfo=0',
                '-C', 'target-feature=+crt-static', '-C', 'link-arg=/PDBALTPATH:ghost-reforged-setup.pdb')
            $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]31
            & $cargo build --locked --release --target $target --manifest-path (Join-Path $PSScriptRoot 'setup\Cargo.toml') --target-dir (Join-Path $buildRoot 'setup')
            if ($LASTEXITCODE -ne 0) { throw 'Windows installer build failed.' }
        } finally { $env:CARGO_ENCODED_RUSTFLAGS = $previousFlags }
        Copy-Item -LiteralPath (Join-Path $buildRoot "setup\$target\release\ghost-reforged-setup.exe") -Destination (Join-Path $release 'GHost-Reforged-Setup-x64.exe') -Force
    }
    $assets = @('ghost-reforged.exe', [IO.Path]::GetFileName($zip), 'LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.txt', 'install.ps1')
    if ($Setup) { $assets += 'GHost-Reforged-Setup-x64.exe' }
    $assets | Sort-Object | ForEach-Object {
        $digest = (Get-FileHash -LiteralPath (Join-Path $release $_) -Algorithm SHA256).Hash.ToLowerInvariant()
        "$digest  $_"
    } | Set-Content -LiteralPath (Join-Path $release 'SHA256SUMS') -Encoding ascii
    Write-Host 'Release assets and SHA256SUMS are ready.'
} finally { Pop-Location }
