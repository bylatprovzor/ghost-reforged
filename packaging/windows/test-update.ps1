$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$audit = Join-Path $repo ('.audit\updater-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $audit | Out-Null
$fixture = @'
using System;
using System.IO;
using System.Threading;
public static class UpdateFixture {
  public static int Main(string[] args) {
    if (Array.IndexOf(args, "--wait-fixture") >= 0) { Thread.Sleep(700); return 0; }
    bool setup = Array.IndexOf(args, "--silent") >= 0;
    File.WriteAllLines(Environment.GetEnvironmentVariable(setup ? "GHOST_SETUP_LOG" : "GHOST_APP_LOG"), args);
    return setup ? int.Parse(Environment.GetEnvironmentVariable("GHOST_SETUP_EXIT")) : 0;
  }
}
'@
$fixtureExe = Join-Path $audit 'fixture.exe'
Add-Type -TypeDefinition $fixture -OutputAssembly $fixtureExe -OutputType ConsoleApplication
foreach ($scenario in @('success', 'installer-failure', 'hash-failure')) {
    $directory = Join-Path $audit ($scenario + ' space')
    $staging = Join-Path $directory 'staging'
    $data = Join-Path $directory 'profiles with spaces'
    New-Item -ItemType Directory -Path $staging, $data -Force | Out-Null
    $application = Join-Path $directory 'ghost-reforged.exe'
    $setup = Join-Path $staging 'GHost-Reforged-Setup-x64.exe'
    Copy-Item -LiteralPath $fixtureExe -Destination $application
    Copy-Item -LiteralPath $fixtureExe -Destination $setup
    Copy-Item -LiteralPath (Join-Path $repo 'src\launcher\update.ps1') -Destination $staging
    Set-Content -LiteralPath (Join-Path $data 'profiles.json') -Value 'preserve-this-profile'
    $env:GHOST_APP_LOG = Join-Path $directory 'application.args'
    $env:GHOST_SETUP_LOG = Join-Path $directory 'setup.args'
    $env:GHOST_SETUP_EXIT = if ($scenario -eq 'installer-failure') { '1' } else { '0' }
    $hash = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash
    if ($scenario -eq 'hash-failure') { $hash = '0' * 64 }
    $parent = Start-Process -FilePath $application -ArgumentList '--wait-fixture' -WindowStyle Hidden -PassThru
    $timer = [Diagnostics.Stopwatch]::StartNew()
    & (Join-Path $staging 'update.ps1') -WaitPid $parent.Id -Application $application -DataDirectory $data -ExpectedHash $hash -ResumeHosting
    if ($timer.ElapsedMilliseconds -lt 500) { throw 'Updater did not wait for the previous process.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while (-not (Test-Path -LiteralPath $env:GHOST_APP_LOG) -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 50
    }
    $received = @(Get-Content -LiteralPath $env:GHOST_APP_LOG)
    if ($received.Count -ne 3 -or $received[0] -ne '--data-dir' -or $received[1] -ne $data -or $received[2] -ne '--host') {
        throw 'Updater did not preserve profile arguments and hosting intent.'
    }
    if ((Get-Content -LiteralPath (Join-Path $data 'profiles.json')) -ne 'preserve-this-profile') {
        throw 'Updater changed the profile.'
    }
    if ($scenario -eq 'hash-failure' -and (Test-Path -LiteralPath $env:GHOST_SETUP_LOG)) { throw 'Unverified installer ran.' }
    if ($scenario -ne 'success' -and -not (Test-Path -LiteralPath (Join-Path $data 'update-error.txt'))) { throw 'Update failure was lost.' }
    if (Test-Path -LiteralPath $staging) { throw 'Updater left its temporary installer behind.' }
    Write-Output "PASS $scenario : process handoff, profile preservation and hosting restart"
}
