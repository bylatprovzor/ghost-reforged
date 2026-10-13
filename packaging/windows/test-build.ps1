param([string]$BuilderScript)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$testRoot = Join-Path $root ('target\build-tests\' + [Guid]::NewGuid().ToString('N'))
$previousTarget = $env:CARGO_TARGET_DIR
$cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
$powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
try {
    $packaging = Join-Path $testRoot 'packaging\windows'
    New-Item -ItemType Directory -Path $packaging -Force | Out-Null
    $builder = if ($BuilderScript) { $BuilderScript } else { Join-Path $PSScriptRoot 'build.ps1' }
    Copy-Item -LiteralPath $builder -Destination (Join-Path $packaging 'build.ps1')
    foreach ($name in @('Install.cmd', 'install.ps1', 'install-online.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $packaging
    }
    foreach ($name in @('README.md', 'LICENSE', 'NOTICE')) {
        [IO.File]::WriteAllText((Join-Path $testRoot $name), 'Packaging fixture')
    }
    [IO.File]::WriteAllText((Join-Path $packaging 'licenses.ps1'), 'param([string]$OutputFile) [IO.File]::WriteAllText($OutputFile, "Fixture notices")')
    [IO.File]::WriteAllText((Join-Path $testRoot 'Cargo.toml'), @'
[package]
name = "ghost-reforged"
version = "0.2.0"
edition = "2024"
[[bin]]
name = "ghost-reforged"
path = "main.rs"
[[bin]]
name = "ghost-reforged-cli"
path = "cli.rs"
'@)
    $source = Join-Path $testRoot 'main.rs'
    [IO.File]::WriteAllText($source, 'fn main() { println!("old"); }')
    $consoleSource = Join-Path $testRoot 'cli.rs'
    [IO.File]::WriteAllText($consoleSource, 'fn main() { println!("old console"); }')
    Push-Location $testRoot
    try {
        & $cargo generate-lockfile
        if ($LASTEXITCODE -ne 0) { throw 'Fixture lockfile failed.' }
        & $cargo build --locked --release --target x86_64-pc-windows-msvc --target-dir (Join-Path $testRoot 'target')
        if ($LASTEXITCODE -ne 0) { throw 'Fixture build failed.' }
        [IO.File]::WriteAllText($source, 'fn main() { println!("new"); }')
        [IO.File]::WriteAllText($consoleSource, 'fn main() { println!("new console"); }')
        $env:CARGO_TARGET_DIR = Join-Path $testRoot 'redirected-target'
        & $powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $packaging 'build.ps1')
        if ($LASTEXITCODE -ne 0) { throw 'Package build failed.' }
        $packagedExe = Join-Path $testRoot 'target\release-assets\0.2.0\ghost-reforged.exe'
        $output = & $packagedExe
        if ($LASTEXITCODE -ne 0 -or $output -ne 'new') { throw 'Build packaged the stale executable.' }
        $packagedConsole = Join-Path $testRoot 'target\release-assets\0.2.0\ghost-reforged-cli.exe'
        $output = & $packagedConsole
        if ($LASTEXITCODE -ne 0 -or $output -ne 'new console') { throw 'Build did not package the current console executable.' }
        $hashes = Get-Content -Raw -LiteralPath (Join-Path $testRoot 'target\release-assets\0.2.0\SHA256SUMS')
        $consoleHash = (Get-FileHash -LiteralPath $packagedConsole -Algorithm SHA256).Hash.ToLowerInvariant()
        if (-not $hashes.Contains("$consoleHash  ghost-reforged-cli.exe")) { throw 'Console checksum is missing.' }
        $payload = Join-Path $testRoot 'target\package\GHost-Reforged-0.2.0-windows-x64'
        $installed = Join-Path $testRoot 'installed'
        & $powershell -NoProfile -File (Join-Path $payload 'install.ps1') -InstallDirectory $installed -NoShortcut
        if ($LASTEXITCODE -ne 0) { throw 'Fixture package installation failed.' }
        $output = & (Join-Path $installed 'ghost-reforged-cli.exe')
        if ($LASTEXITCODE -ne 0 -or $output -ne 'new console') { throw 'Installer omitted the console executable.' }
        Write-Output 'PASS: External CARGO_TARGET_DIR cannot make the builder package an old executable.'
        Write-Output 'PASS: Console executable and its checksum are included in release assets.'
        Write-Output 'PASS: Package installation includes the current console executable.'
    } finally { Pop-Location }
} finally {
    $env:CARGO_TARGET_DIR = $previousTarget
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $allowed = [IO.Path]::GetFullPath((Join-Path $root 'target\build-tests')) + '\'
    if ($resolved.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolved)) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
