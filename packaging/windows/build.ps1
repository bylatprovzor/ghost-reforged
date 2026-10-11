param([switch]$NoBuild)
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
            & $cargo build --locked --release --target $target --bin ghost-reforged
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
} finally { Pop-Location }
