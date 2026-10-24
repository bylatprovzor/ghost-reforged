param(
    [Parameter(Mandatory=$true)][string]$DataDirectory,
    [Parameter(Mandatory=$true)][string]$Python
)
$ErrorActionPreference='Stop'
$data=[IO.Path]::GetFullPath($DataDirectory).TrimEnd('\','/')
if ($data -match '^[A-Za-z]:$') { throw 'Bot data must be in a dedicated folder.' }
$runtime=[IO.Path]::GetFullPath($Python)
$runtime=Join-Path ([IO.Path]::GetDirectoryName($runtime)) 'pythonw.exe'
$script=Join-Path $PSScriptRoot 'maintain-runtime.py'
if (!(Test-Path -LiteralPath (Join-Path $data 'profiles.json') -PathType Leaf)) { throw 'Configured bot data is required.' }
if (!(Test-Path -LiteralPath $runtime -PathType Leaf) -or !(Test-Path -LiteralPath $script -PathType Leaf)) { throw 'Maintenance runtime or script is missing.' }
foreach ($path in @($data,$runtime,$script)) { if ($path -match '["\r\n]') { throw 'Invalid maintenance path.' } }
$arguments='"'+$script+'" --state-dir "'+$data+'" --apply'
$taskName='GHostReforged-Maintenance'
$service=New-Object -ComObject Schedule.Service
$service.Connect()
$folder=$service.GetFolder('\')
$existing=@($folder.GetTasks(1) | Where-Object {$_.Name -ieq $taskName})
if ($existing.Count) {
    [xml]$xml=$existing[0].Xml
    if ($xml.Task.Actions.Exec.Command -ine $runtime -or $xml.Task.Actions.Exec.Arguments -cne $arguments) {
        throw 'A different maintenance task exists; it was not changed.'
    }
}
$task=$service.NewTask(0)
$task.RegistrationInfo.Description='Hourly bounded cleanup of generated Warcraft and Battle.net diagnostics; maps, replays and match data are excluded.'
$task.Principal.UserId=[Security.Principal.WindowsIdentity]::GetCurrent().Name
$task.Principal.LogonType=3
$task.Principal.RunLevel=0
$task.Settings.Enabled=$true
$task.Settings.Hidden=$true
$task.Settings.StartWhenAvailable=$true
$task.Settings.DisallowStartIfOnBatteries=$false
$task.Settings.StopIfGoingOnBatteries=$false
$task.Settings.MultipleInstances=2
$task.Settings.ExecutionTimeLimit='PT5M'
# The scheduler defaults to below-normal CPU and low I/O priority. A busy game
# can then prevent even a small cleanup from finishing within its time limit.
$task.Settings.Priority=4
$trigger=$task.Triggers.Create(1)
$trigger.StartBoundary=(Get-Date).AddMinutes(1).ToString('yyyy-MM-ddTHH:mm:ss')
$trigger.Repetition.Interval='PT1H'
$trigger.Enabled=$true
$action=$task.Actions.Create(0)
$action.Path=$runtime
$action.Arguments=$arguments
$action.WorkingDirectory=$PSScriptRoot
$null=$folder.RegisterTaskDefinition($taskName,$task,6,$task.Principal.UserId,$null,3)
@{task=$taskName;interval_minutes=60;preserve_recent_hours=48;maximum_age_days=14;status_file=(Join-Path $data 'maintenance-status.json')}|ConvertTo-Json
