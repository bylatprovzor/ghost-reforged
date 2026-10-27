param(
    [Parameter(Mandatory = $true)][string]$DataDirectory,
    [string]$ExecutableDirectory = ''
)
$ErrorActionPreference = 'Stop'
if (-not $ExecutableDirectory) {
    $ExecutableDirectory = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
}
# Deliberately allowlist diagnostic fields. Never print profiles, command lines,
# account names, endpoints, raw native frames, or Battle.net configuration.
$data = [IO.Path]::GetFullPath($DataDirectory)
$root = [IO.Path]::GetFullPath($ExecutableDirectory)
$cli = Join-Path $root 'ghost-reforged-cli.exe'
if (-not (Test-Path -LiteralPath $cli -PathType Leaf)) { throw 'Controller executable is missing.' }

function Invoke-Control([string]$Command) {
    $oldPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $text = & $cli --data-dir $data $Command 2>$null
        if ($LASTEXITCODE -eq 0) { return ($text | ConvertFrom-Json) }
    } catch { } finally { $ErrorActionPreference = $oldPreference }
    return $null
}

$os = Get-CimInstance Win32_OperatingSystem
$machine = Get-CimInstance Win32_ComputerSystem
$current = Get-Process -Id $PID
$health = Invoke-Control 'health'
$profile = Invoke-Control 'profile-get'
$map = $null
if ($profile -and $profile.map) {
    try {
        $file = Get-Item -LiteralPath $profile.map
        $map = [ordered]@{name=$file.Name;bytes=$file.Length;sha256=(Get-FileHash -LiteralPath $file.FullName).Hash}
    } catch { $map = [ordered]@{available=$false} }
}
$binaryHashes = [ordered]@{}
foreach ($name in @('ghost-reforged-supervisor.exe','ghost-reforged-cli.exe')) {
    $file = Join-Path $root $name
    if (Test-Path -LiteralPath $file -PathType Leaf) {
        $binaryHashes[$name] = (Get-FileHash -LiteralPath $file).Hash
    }
}
$names = @('ghost-reforged-supervisor','ghost-reforged-cli','Warcraft III','Battle.net','BlizzardBrowser')
$processes = @(Get-Process -Name $names -ErrorAction SilentlyContinue | ForEach-Object {
    try {
        [ordered]@{
            name=$_.ProcessName
            pid=$_.Id
            birth=[string]$_.StartTime.ToUniversalTime().ToFileTimeUtc()
            session=$_.SessionId
            priority=[string]$_.PriorityClass
            cpu_seconds=[math]::Round($_.CPU,2)
            private_mib=[math]::Round($_.PrivateMemorySize64/1MB)
            working_set_mib=[math]::Round($_.WorkingSet64/1MB)
        }
    } catch { }
})
$sessions = @()
try {
    if (-not ('GHost.SessionFacts' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace GHost {
    public static class SessionFacts {
        [DllImport("wtsapi32.dll", EntryPoint="WTSQuerySessionInformationW")]
        static extern bool Query(IntPtr server, int session, int kind, out IntPtr buffer, out int bytes);
        [DllImport("wtsapi32.dll")]
        static extern void WTSFreeMemory(IntPtr buffer);
        public static int Read(int session, int kind) {
            IntPtr buffer = IntPtr.Zero;
            int bytes;
            try {
                if (!Query(IntPtr.Zero, session, kind, out buffer, out bytes)) return -1;
                if (kind == 16 && bytes >= 2) return (ushort)Marshal.ReadInt16(buffer);
                if (kind == 8 && bytes >= 4) return Marshal.ReadInt32(buffer);
                return -1;
            } finally { if (buffer != IntPtr.Zero) WTSFreeMemory(buffer); }
        }
    }
}
'@
    }
    $sessions = @($processes.session | Sort-Object -Unique | ForEach-Object {
        $state = [GHost.SessionFacts]::Read($_, 8)
        $protocol = [GHost.SessionFacts]::Read($_, 16)
        [ordered]@{
            id=$_;state_code=$state
            state=switch($state){0{'active'}4{'disconnected'}default{'other_or_unknown'}}
            transport=switch($protocol){0{'console'}2{'rdp'}default{'other_or_unknown'}}
        }
    })
} catch { $sessions = @([ordered]@{available=$false}) }
$task = Get-ScheduledTask -TaskName 'GHostReforged-Supervisor' -ErrorAction SilentlyContinue
$taskReport = [ordered]@{present=($null -ne $task)}
if ($task) {
    $expectedArguments = '--data-dir "' + $data.TrimEnd('\') + '"'
    $taskReport.state = [string]$task.State
    $taskReport.interactive = [string]$task.Principal.LogonType -eq 'Interactive'
    $taskReport.unlimited = [string]$task.Settings.ExecutionTimeLimit -eq 'PT0S'
    $taskReport.duplicate_policy = [string]$task.Settings.MultipleInstances
    $taskReport.restart_count = $task.Settings.RestartCount
    $taskReport.at_logon = @($task.Triggers | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_TaskLogonTrigger' }).Count -gt 0
    $taskReport.action_matches = @($task.Actions).Count -eq 1 -and
        $task.Actions[0].Execute -ieq (Join-Path $root 'ghost-reforged-supervisor.exe') -and
        $task.Actions[0].Arguments -ceq $expectedArguments
}
$preferences = [ordered]@{}
$preferencePath = Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'Warcraft III\War3Preferences.txt'
if (Test-Path -LiteralPath $preferencePath -PathType Leaf) {
    $section = ''
    foreach ($line in Get-Content -LiteralPath $preferencePath) {
        if ($line -match '^\[([A-Za-z0-9]+)\]') { $section = $Matches[1]; continue }
        if ($line -match '^\s*(maxfps|backgroundmaxfps|windowmode|windowwidth|windowheight|reswidth|resheight|vsync|seenintromovie|lightingquality|particles|pointlightshadowquality|shadowquality|texquality|spellfilter|music|sfx|musicvolume|sfxvolume)\s*=\s*([0-9]+)\s*$') {
            $preferences[$section + '.' + $Matches[1]] = [int]$Matches[2]
        }
    }
}
$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$($os.SystemDrive)'"
$winlogon = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon' -Name AutoAdminLogon -ErrorAction SilentlyContinue
$report = [ordered]@{
    schema_version=1
    utc=[DateTime]::UtcNow.ToString('o')
    windows=[ordered]@{
        version=$os.Version;build=$os.BuildNumber;server=($os.ProductType -ne 1)
        boot_utc=$os.LastBootUpTime.ToUniversalTime().ToString('o')
        logical_processors=$machine.NumberOfLogicalProcessors
        memory_mib=[math]::Round($machine.TotalPhysicalMemory/1MB)
        available_mib=[math]::Round($os.FreePhysicalMemory/1024)
        system_disk_free_gib=[math]::Round($disk.FreeSpace/1GB,2)
        diagnostic_session=$current.SessionId
        automatic_sign_in_configured=($winlogon.AutoAdminLogon -eq '1')
    }
    graphics=@(Get-CimInstance Win32_VideoController | ForEach-Object {
        [ordered]@{name=$_.Name;driver_version=$_.DriverVersion;status=$_.Status}
    })
    audio=[ordered]@{
        services=@(Get-Service -Name Audiosrv,AudioEndpointBuilder | ForEach-Object {
            [ordered]@{name=$_.Name;status=[string]$_.Status;start_type=[string]$_.StartType}
        })
        devices=@(Get-CimInstance Win32_SoundDevice | ForEach-Object {
            [ordered]@{name=$_.Name;status=$_.Status;problem_code=$_.ConfigManagerErrorCode}
        })
    }
    binaries=$binaryHashes
    supervisor_task=$taskReport
    controller=$null
    profile=$null
    map=$map
    game_preferences=$preferences
    processes=$processes
    sessions=$sessions
}
if ($health) {
    $report.controller = [ordered]@{
        version=$health.version;pid=$health.pid;hosting_requested=$health.hosting_requested;quitting=$health.quitting;worker=$null
    }
    if ($health.worker) {
        $report.controller.worker = [ordered]@{
            stage=$health.worker.stage;age_seconds=$health.worker.age_seconds;stall_after_seconds=$health.worker.stall_after_seconds
        }
    }
}
if ($profile) {
    $report.profile = [ordered]@{
        headless=$profile.headless;minimize_window=$profile.minimize_window;rehost=$profile.rehost;private=$profile.private_game
        next_number=$profile.next_number;minimum_players=$profile.players;require_full=$profile.require_full
        startup_timeout_seconds=$profile.startup_timeout_seconds;creation_timeout_seconds=$profile.creation_timeout_seconds
    }
}
$report | ConvertTo-Json -Depth 7
