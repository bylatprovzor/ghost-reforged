param([string]$Version = '0.2.5', [string]$OnlineScript)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$release = Join-Path $root "target\release-assets\$Version"
$installer = Join-Path $release 'GHost-Reforged-Setup-x64.exe'
$global:GHostInstallFixture = @{ Release = $release; Installer = $installer; Tag = "v$Version"; Mode = 'valid' }
$testRoot = Join-Path $root ('target\install-tests\' + [Guid]::NewGuid().ToString('N'))
$fixture = $null
$previousArgumentsOutput = $env:GHOST_INSTALL_ARGUMENTS_OUTPUT

function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Invoke-Setup([string]$Directory, [switch]$Shortcut, [int]$ExpectedExit = 0) {
    $arguments = @('--silent', '--no-launch', '--install-dir', ('"' + $Directory + '"'))
    if (-not $Shortcut) { $arguments += '--no-shortcut' }
    $process = Start-Process -FilePath $installer -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    Assert-True ($process.ExitCode -eq $ExpectedExit) "Unexpected installer exit: $($process.ExitCode)."
}

# Replace only the download boundary; the downloaded installer runs normally.
function Invoke-WebRequest {
    param([string]$Uri, [string]$OutFile, [switch]$UseBasicParsing)
    $base = 'https://github.com/shideneyu/ghost-reforged/releases/download/' + $global:GHostInstallFixture.Tag + '/'
    Assert-True ($Uri.StartsWith($base, [StringComparison]::Ordinal)) 'Unexpected download URL.'
    $name = $Uri.Substring($base.Length)
    Assert-True ($name -in @('SHA256SUMS', 'GHost-Reforged-Setup-x64.exe')) 'Unexpected release asset.'
    Assert-True (-not [string]::IsNullOrEmpty($OutFile)) 'Download must use a file for binary content types.'
    if ($name -eq 'SHA256SUMS') {
        $checksums = Get-Content -Raw -LiteralPath (Join-Path $global:GHostInstallFixture.Release $name)
        if ($global:GHostInstallFixture.Mode -eq 'mismatch') {
            $checksums = $checksums -replace '(?m)^[a-f0-9]{64}(  GHost-Reforged-Setup-x64\.exe)', (('0' * 64) + '$1')
        } elseif ($global:GHostInstallFixture.Mode -eq 'missing') {
            $checksums = ''
        } elseif ($global:GHostInstallFixture.Mode -eq 'duplicate') {
            $checksums += "`n" + $checksums
        }
        [IO.File]::WriteAllText($OutFile, $checksums)
    } else {
        Copy-Item -LiteralPath $global:GHostInstallFixture.Installer -Destination $OutFile
    }
}

function Invoke-RestMethod {
    param([string]$Uri)
    Assert-True ($Uri -eq 'https://api.github.com/repos/shideneyu/ghost-reforged/releases/latest') 'Unexpected API request.'
    [pscustomobject]@{ tag_name = $global:GHostInstallFixture.Tag }
}

$programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
$shortcutPath = Join-Path $programs 'GHost Reforged.lnk'
$originalShortcut = if (Test-Path -LiteralPath $shortcutPath) { [IO.File]::ReadAllBytes($shortcutPath) } else { $null }
try {
    Assert-True (Test-Path -LiteralPath $installer -PathType Leaf) 'Build the setup executable first.'
    New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
    $destination = Join-Path $testRoot "Offline [test] O'Brien"
    Invoke-Setup $destination
    $installedExe = Join-Path $destination 'ghost-reforged.exe'
    $expectedHash = (Get-FileHash -LiteralPath (Join-Path $release 'ghost-reforged.exe')).Hash
    Assert-True ((Get-FileHash -LiteralPath $installedExe).Hash -eq $expectedHash) 'Installed executable differs from release.'
    foreach ($name in @('ghost-reforged-cli.exe', 'README.md', 'LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.txt')) {
        Assert-True (Test-Path -LiteralPath (Join-Path $destination $name)) "Missing installed file: $name."
    }
    $sentinel = Join-Path $destination 'profiles.json'
    [IO.File]::WriteAllText($sentinel, '{"preserve":true}')
    Invoke-Setup $destination -Shortcut
    Assert-True ((Get-Content -Raw -LiteralPath $sentinel) -eq '{"preserve":true}') 'Reinstall modified user data.'
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($shortcutPath)
        try {
            Assert-True ($shortcut.TargetPath -ieq $installedExe) 'Start menu shortcut has the wrong target.'
            Assert-True ($shortcut.WorkingDirectory -ieq $destination) 'Start menu shortcut has the wrong working directory.'
        } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) }
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) }
    Write-Output 'PASS: Offline install, shortcut, reinstall, spaces and literal brackets; user data preserved.'

    $lockedDirectory = Join-Path $testRoot 'Running application'
    New-Item -ItemType Directory -Path $lockedDirectory | Out-Null
    $lockedExe = Join-Path $lockedDirectory 'ghost-reforged.exe'
    Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\ping.exe') -Destination $lockedExe
    $lockedHash = (Get-FileHash -LiteralPath $lockedExe).Hash
    $fixture = Start-Process -FilePath $lockedExe -ArgumentList @('-t', '127.0.0.1') -WindowStyle Hidden -PassThru
    Invoke-Setup $lockedDirectory -ExpectedExit 1
    Assert-True ((Get-FileHash -LiteralPath $lockedExe).Hash -eq $lockedHash) 'Running executable was overwritten.'
    Assert-True (-not $fixture.HasExited) 'Installer stopped the running application.'
    Write-Output 'PASS: Updating a running application fails without stopping or overwriting it.'

    $online = if ($OnlineScript) { $OnlineScript } else { Join-Path $PSScriptRoot 'install-online.ps1' }
    $global:GHostInstallFixture.Mode = 'valid'
    $onlineDirectory = Join-Path $testRoot 'Online [test]'
    & $online -InstallDirectory $onlineDirectory -NoShortcut
    Assert-True ((Get-FileHash -LiteralPath (Join-Path $onlineDirectory 'ghost-reforged.exe')).Hash -eq $expectedHash) 'Online installation differs from release.'
    & $online -Version latest -InstallDirectory $onlineDirectory -NoShortcut
    Write-Output 'PASS: Pinned and latest online install with file-based checksum download.'
    foreach ($mode in @('mismatch', 'missing', 'duplicate')) {
        $global:GHostInstallFixture.Mode = $mode
        $rejectedDirectory = Join-Path $testRoot $mode
        $rejected = $false
        try { & $online -InstallDirectory $rejectedDirectory -NoShortcut } catch { $rejected = $true }
        Assert-True $rejected "Installer accepted $mode checksums."
        Assert-True (-not (Test-Path -LiteralPath $rejectedDirectory)) 'Rejected download wrote installation files.'
    }
    Write-Output 'PASS: Incorrect, missing and duplicate checksums prevent installation.'

    $global:GHostInstallFixture.Mode = 'valid'
    $global:GHostInstallFixture.ArgumentProbe = Join-Path $testRoot 'argument-probe.exe'
    $env:GHOST_INSTALL_ARGUMENTS_OUTPUT = Join-Path $testRoot 'arguments.txt'
    Add-Type -OutputAssembly $global:GHostInstallFixture.ArgumentProbe -OutputType ConsoleApplication -TypeDefinition @'
using System;
using System.IO;
class Arguments {
    static void Main(string[] args) {
        File.WriteAllLines(Environment.GetEnvironmentVariable("GHOST_INSTALL_ARGUMENTS_OUTPUT"), args);
    }
}
'@
    function Start-Process {
        param($FilePath, $ArgumentList, $WindowStyle, [switch]$Wait, [switch]$PassThru)
        Assert-True ([IO.Path]::GetFileName($FilePath) -eq 'GHost-Reforged-Setup-x64.exe') 'Unexpected installer process.'
        Microsoft.PowerShell.Management\Start-Process -FilePath $global:GHostInstallFixture.ArgumentProbe -ArgumentList $ArgumentList -WindowStyle Hidden -Wait -PassThru
    }
    foreach ($directory in @([IO.Path]::GetPathRoot($testRoot), ($destination + '\'))) {
        & $online -InstallDirectory $directory -NoShortcut
        $received = [IO.File]::ReadAllLines($env:GHOST_INSTALL_ARGUMENTS_OUTPUT)
        Assert-True ($received.Count -eq 4 -and $received[2] -eq '--install-dir' -and $received[3] -ceq $directory) 'Installer destination changed across the native process boundary.'
    }
    Write-Output 'PASS: Drive-root and trailing-separator arguments reach the native process unchanged.'
} finally {
    $env:GHOST_INSTALL_ARGUMENTS_OUTPUT = $previousArgumentsOutput
    Remove-Variable -Name GHostInstallFixture -Scope Global -ErrorAction SilentlyContinue
    if ($fixture -and -not $fixture.HasExited) { $fixture.Kill(); $fixture.WaitForExit() }
    if ($null -ne $originalShortcut) {
        [IO.File]::WriteAllBytes($shortcutPath, [byte[]]$originalShortcut)
    } elseif (Test-Path -LiteralPath $shortcutPath) {
        Remove-Item -LiteralPath $shortcutPath -Force
    }
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $allowed = [IO.Path]::GetFullPath((Join-Path $root 'target\install-tests')) + '\'
    if ($resolved.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolved)) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
