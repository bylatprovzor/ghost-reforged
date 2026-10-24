# Windows hosting

The configuration below has passed native cold-boot, controller-crash,
loading-to-rehost and twenty-minute idle checks on a Windows VPS without RDP.
A process or socket alone is not a successful launch: acceptance requires the
expected native lobby and a healthy controller.

## Required state

- Install Battle.net and the supported Warcraft III build under the same Windows
  user that runs GHost. Sign in to Battle.net and enable its remembered session.
  The current verified build is `3.0.0.24268`. Game updates require a new
  compatibility check; an unverified build blocks hosting.
- The recorded launcher version is Battle.net `2.53.5.17932`. Keep launcher
  version changes in the deployment record as well as game updates.
- Keep that Windows user logged on. GHost's task uses an interactive session;
  starting the game from an SSH service in session 0 is not equivalent.
- For recovery after an operating-system reboot, configure automatic Windows
  sign-in separately. Disconnecting RDP must not log off the hosting user.
- Select a map in that user's Warcraft III maps directory and save its profile.
  Retain the profile directory, counter and reporting queue across upgrades.
- Keep the complete Windows bundle together. Record executable and map hashes.
- Provide a persistent audio output in the hosting session, even with muted
  game audio. An RDP audio device can disappear after disconnecting or rebooting.

Headless hosting still runs Warcraft's native engine and embedded browser. It
does not remove Direct3D, graphics-driver, Windows-session or Battle.net
dependencies. The verified GPU-less trial retained the original game libraries
and used a persistent audio device and an automatically opened console session.

An absent audio device can stop Warcraft before its local API opens, displaying
`Unable to initialize audio device`. Muting the game and setting `nosoundwarn=1`
did not avoid this dialog in the tested build. A running Windows Audio service
alone is insufficient. Inspect the audio devices in `diagnose.ps1` before
attributing a pre-API stall to the GPU.

A persistent virtual audio driver is a possible prerequisite on servers without
sound hardware. The current native trial uses the official
[VB-CABLE package](https://vb-audio.com/Cable/) (donationware), installed
separately and rebooted according to its instructions. It is not included in the
GHost bundle. After a full reboot without RDP, the tested device remained
available and Warcraft reached authentication, online multiplayer and a stable
lobby. Muted preferences did not replace this audio-device requirement.

The verified trial used `VBCABLE_Driver_Pack45.zip`, driver version `3.3.1.7`,
with SHA-256
`B950E39F01AF1D04EA623C8F6D8EB9B6EA5C477C637295FABF20631C85116BFB`.
The original installer had a valid publisher signature and its Windows 10
driver catalog was signed by Microsoft Windows Hardware Compatibility
Publisher. No certificate-store or driver-signing-policy changes were needed.
GHost checks for an audio output before requesting a new game; native testing
confirmed that stopping Windows Audio produces an explicit refusal without
starting Warcraft, and that the audio service can be restored afterward.

After the game has created its preferences, close it and inspect the dedicated
hosting preset. Apply it once; running it again preserves the same settings.
It uses 1024×768, 20 FPS / 5 background FPS, low graphics, muted audio and skips
the already-seen introduction. Unrelated preferences remain unchanged; changes
retain an adjacent `.backup` file. It does not replace graphics libraries.

```powershell
.\configure-host.ps1
.\configure-host.ps1 -Apply
```

Record the native runtime before starting a trial. Set `$game` to the installed
`_retail_\x86_64` directory, then retain the report with the bundle manifest:

```powershell
.\inspect-runtime.ps1 -GameDirectory $game -VerifySignatures > runtime.json
```

This hashes an explicit list of game and browser files, reports missing files
and checks for adjacent Direct3D replacement libraries. It does not publish the
installation path or read account configuration. Run the full inventory while
the game is stopped; hashing and signature validation can compete with startup
on a small server. Missing files and unreadable files are not a successful check.

The verified deployment uses the original Blizzard browser and SwiftShader
libraries; their hashes match the retained originals. No extra game launch
arguments or adjacent Direct3D replacement libraries are configured.
An unsigned bundled library alone does not establish that it was modified:
compare its hash with a known original of the same game build.

## Install the resident supervisor

To build the compact server bundle from source, run
`packaging\windows\build-host.ps1`. It builds only the controller and CLI,
includes these scripts and writes a file manifest and ZIP checksum. Maps,
profiles and credentials are not bundled. A dirty source tree is reported in
the manifest rather than presented as an official release.

Run this from the extracted bundle in the hosting user's Windows session. Set
`$data` to the directory containing the already configured `profiles.json`.

```powershell
$data = (Resolve-Path .\data).Path
.\supervisor.ps1 -DataDirectory $data -Start
```

Apply server settings to the selected profile while it is stopped. Save this
JSON as `server-settings.json`, then run the command below. It changes only these
fields and preserves the map, password, counter and other profile settings.

```json
{
  "headless": true,
  "minimize_window": false,
  "startup_timeout_seconds": 600,
  "creation_timeout_seconds": 180,
  "rehost": true,
  "rehost_delay_seconds": 15,
  "empty_rehost_seconds": 0
}
```

```powershell
.\ghost-reforged-cli.exe --data-dir $data profile-set --file .\server-settings.json
.\ghost-reforged-cli.exe --data-dir $data host
```

For a new profile directory, `profile-set --file` also works while no controller
is running: it creates the default store and merges your settings. Include the
map, name pattern, privacy and password in that initial local JSON, then install
the supervisor. For example, set `$data = Join-Path (Get-Location) 'data'` before
the first command instead of resolving an existing directory. Keep local
settings and the data directory out of shared bundles.

The supervisor adopts its own controller or starts one, checks worker progress,
and backs off after repeated failures. The main scheduled task starts it at sign-in.
A separate task checks its native lock every minute and restores a missing
supervisor even if Windows does not restart its task. A surviving controller is adopted.
Recovery checks can overlap; the exclusive lock prevents duplicate supervisors.
Explicit `stop` remains stopped after a restart; `quit` shuts down the controller
and supervisor and disables recurring recovery until an explicit restart.
Neither command is a request to log off Windows.

Native lobby departure can take longer than the Stop acknowledgement deadline
on a busy server. In that case the controller reports unconfirmed closure and
keeps hosting stopped; it does not claim that Warcraft exited. Verify the game
state before replacing files. The private acceptance cleanup retains the exact
test process and closes it after stopping the empty test lobby.

```powershell
.\ghost-reforged-cli.exe --data-dir $data health
.\ghost-reforged-cli.exe --data-dir $data stop
.\ghost-reforged-cli.exe --data-dir $data host
```

## Startup checkpoints

### Ranked ladder credentials

Ranked session registration and game reporting need the collector credential.
For unattended Windows hosting, keep it in a DPAPI-protected file readable only
by the hosting user and the administrators who maintain the receiver. A file
protected on another machine is not a portable backup of that credential.
Provision it separately when restoring to a new machine.

Run the following as the hosting user, with `$credential` pointing to the
existing protected file. The script validates it and persists only its path in
the user's environment. It neither prints nor changes the credential.

```powershell
.\configure-ladder.ps1 -ProtectedCredentialPath $credential
.\configure-ladder.ps1 -ProtectedCredentialPath $credential -Apply
```

Sign in again or reboot afterward, so the scheduled supervisor inherits
`W3LADDER_COLLECTOR_KEY_FILE`. GHost decrypts the file when needed, without a
dialog, and does not pass the plaintext credential to Warcraft. The existing
`W3LADDER_COLLECTOR_KEY` process override still takes precedence. Invalid or
unreadable configured credentials fail closed; they are never silently replaced.
Keep `W3LADDER_MODE` unset or `ranked` for real hosting. `test` and `preview`
cannot substitute for a real ranked setup.

### Native readiness

Local render setup is retried for up to 30 seconds before sending creation.
Transient guardian startup or child-process failures do not immediately discard
an already authenticated socket. Stop cancels this wait. A persistent failure
still enters recovery, and the operational journal distinguishes an expired
acknowledgment, a failed native operation and an exited guardian.

1. The expected Windows user has an interactive session and an audio output.
2. The supervisor and its owned controller are alive in that session.
3. Battle.net's current process has completed launcher initialization. A cold
   start opens Battle.net first, then sends one game launch request. An old log
   or an expired process lifetime cannot authorize that request.
4. Exactly one supported Warcraft process exposes its local API.
5. Native callbacks confirm authentication and multiplayer availability. If
   the login screen was observed, its completion must also be acknowledged by
   a native menu callback. Online readiness can arrive before that callback.
   On a newly launched client, the native menu owns provider initialization;
   GHost observes its result rather than issuing a second initialization during
   sign-in. This startup state survives discovery and first-handshake retries;
   finding a listener is not proof that native initialization finished.
   Reconciliation of an already initialized client retains the
   controller's explicit initialization path.
6. Native creation callbacks and a fresh, owned roster agree on the selected
   map and lobby. Online creation may omit its explicit success callback; a
   started callback followed by the lobby screen is also checked against the
   actual roster. A later rejection still invalidates this attempt.
7. The same lobby remains usable for at least five minutes. A momentary lobby
   callback is insufficient: a failed creation can publish a roster late.

Slot edits wait until a fresh roster confirms the host's observer placement.
In particular, an empty observer destination must not be closed while the
native host move is still pending. Manual edits recheck this before sending;
reserved computer restoration uses the same placement barrier.

In headless mode the embedded browser is paused immediately before native lobby
creation, after online readiness and counter persistence. Authentication keeps
the browser active. A separate guardian owns the suspension and releases it if
the controller exits or its short lease expires. Creation failure, sign-out,
game start and departure restore the browser before further native transitions.
This does not stop the native game engine or promise zero CPU use.
The creation lease keeps renewing while Windows minimizes the lobby window;
a slow window transition must not silently release it.

The saved `minimize_window` profile field defaults to `true`. Set it to `false`
while hosting is stopped for a server whose native window cannot reliably
acknowledge minimize/restore requests. The game remains hidden and muted, and
headless mode still pauses its embedded browser; the native engine is not
minimized. This is an explicit deployment setting, preserved across reboots.
This setting passed the current cold-boot and stable-lobby trial. It avoids an
observed window-mode acknowledgment failure; it does not guarantee low total
CPU use. The browser guard still suspends rendering during the owned lobby.

Restoring the ranked map's reserved computer seats sends one batch and waits
for the actual roster to confirm it. Unchanged or partial callbacks do not
reissue those edits. The configured creation timeout also bounds this wait's
warning; a late confirmation is still accepted and incomplete seats block start.
Ladder packet edits also wait for their actual color, race, team and handicap
values. Partial callbacks do not acknowledge a batch. The configured creation
budget replaces the former eight-second edit limit; total preparation remains
bounded by the greater of that budget and two minutes, then falls back to an
unranked game only after clearing the marker.

`LOGIN_DOORS` can precede a successful automatic login. An empty game list does
not prove authentication. A server queue needs time; an unresponsive endpoint
needs a bounded recovery. Never repeat launch requests just because startup is
slow: Battle.net may still have one queued.

An explicit `LoggedOut` event is different from waiting in the initial login
queue. Social events can keep arriving after logout; they must not extend that
disconnected session indefinitely. Reconnecting the local API is an attempt to
recover, not proof that authentication has returned.

Before any lobby use, a newly launched client can be restarted through Battle.net
after the startup budget in either of two confirmed states: an explicit native
sign-out on the login or main menu, or explicit negative authentication while
still on `LOGIN` or `LOGIN_DOORS`. The second case covers an initial login that
never emits `LoggedOut`. Missing authentication data and social traffic alone
never authorize a reset. Repeated negative replies do not restart the timer.

Recovery requires retained ownership of that exact process. The controller
requests current state and drains callbacks for three seconds before reset,
including a quiet final half-second. A late authenticated or online reply cancels
reset. An unanswered getter cannot extend the confirmed failure indefinitely.
Socket reconnection does not erase that deadline. Any creation request,
lobby roster, countdown or loading screen permanently revokes this startup-only
recovery for that process. Existing clients are never claimed this way.

A separate recovery covers the first creation on that newly requested client:
native creation must have started and then explicitly emitted a departure,
without ever publishing a usable roster or successful creation result. The
observed unhosted placeholder (one unnamed empty slot 0, team 255, no observer
or self) is accepted only after creation starts; it does not authorize reset
without the later native departure. Pending callbacks are drained before reset.
Any other roster, join, countdown, loading, unknown screen or second creation
request permanently excludes this path. A socket or an empty listing cannot
authorize it. Restart preserves the existing number reservation.

If creation loses authentication or is rejected, GHost retains that request's
identity. Restart requires a new native confirmation of the exact empty lobby,
with host authority and no other human participant, including observers.
Countdown or loading evidence revokes this recovery permission. Successful
socket connections alone do not reset the retry backoff; a fresh owned lobby
must remain healthy for a minute.

A socket reconnect in a previously idle, owned lobby adopts a fresh matching
roster without cancelling it. A pending or newly observed countdown retains
the cancellation path; repeated socket failures preserve its deadline. Loading
and results callbacks remain authoritative during reconciliation.

An unanswered start command uses the command timeout (30 seconds by default).
Once Warcraft confirms an active countdown, the first confirmation starts the
loading budget (180 seconds by default, never shorter than the command budget).
Repeated callbacks cannot extend it. A roster change still requests cancellation
immediately; that request has its own short acknowledgement deadline. The host
process is replaced only after native loading confirmation.

Each submitted welcome produces a bounded diagnostic with milliseconds from
player detection to the native socket send. This includes queueing and any
requested statistics lookup; it does not measure delivery to the player's UI.
Player names and message contents are excluded from this timing diagnostic.

Manual slot changes wait behind an in-flight slot command, then take priority
over further automatic observer cleanup. The bounded queue rechecks the target
and lobby before sending anything; a new occupant, countdown, changed lobby or
expired request cancels that edit. Command acceptance is not native confirmation:
inspect the subsequent slot activity and roster.

## Capture comparable evidence

```powershell
.\diagnose.ps1 -DataDirectory $data > before.json
# Restart Windows, then repeat from the same bundle and profile directory.
.\diagnose.ps1 -DataDirectory $data > after.json
```

The report includes versions, boot time, process lifetimes and sessions, task
settings, resource usage, selected profile controls and file hashes. It omits
passwords, account names, endpoints, raw frames and launcher configuration.
The diagnostic command reads state; it does not restart the game or change it.

The supervisor journal and hosting journal are bounded. Keep the failed run's
report before changing one variable. Record its binary hashes, map hash, native
state, elapsed time and outcome. Unknown authentication remains unknown.

## Acceptance checks

Use a password-protected lobby before enabling public hosting. Record a fresh
result for each scenario; automated Rust tests do not replace native evidence.

| Scenario | Required result |
| --- | --- |
| Cold Windows boot, no RDP | One game authenticates and opens the correct lobby |
| Controller crash in an empty lobby | Same game, lobby and counter are adopted |
| Worker stops progressing | Supervisor detects the expired lease and recovers |
| Battle.net disconnect | Recovery restores a fresh online session without duplicate games |
| Countdown cancellation | Roster observation and automatic start resume |
| Native loading begins | Start is recorded and the next lobby is eventually created |
| Explicit Stop, then reboot | Hosting remains stopped |
| RDP disconnect and extended idle | API, hosting, memory and remote administration remain usable |

The following 2026-10-15 checks used supervisor SHA-256
`C9ABD13197B2274A80B545C89551F8385D0AFB18A3DF787F2BCEA76E533D9301`
and CLI SHA-256
`700181A7F97A6277524CCEE361F697295271D7AC7CEFED981B97FB41327FD529`.
They do not establish compatibility with arbitrary game updates or server images.

A full cold boot without RDP produced the selected ranked map, the host in
observer, twelve reserved computers and four open player seats. The first
client signed out after authenticating; the bot replaced it automatically
after the bounded recovery wait. The lobby appeared 925 seconds after the
collector started and remained healthy for 311 seconds. Only one Warcraft
process was present, in the automatically opened console session. Killing the
controller then recovered the same game lifetime, lobby and counter in 9.7
seconds. The subsequent 1,226-second idle observation kept that same game and
lobby, with no unhealthy sample among 40 observations.

The same binaries completed a four-computer native cycle at 09:12 UTC:
the start appeared once in Activity, loading triggered client replacement, and
the next lobby remained stable for 62 seconds. Loading began 17 seconds after
the start request; the next lobby opened 163 seconds after loading. This cycle
therefore validates the normal path, while the longer countdown is covered by
the captured failure and deterministic regression. The counter advanced twice,
cleanup completed and the ranked-map profile was restored. Total trial time
was 1,345 seconds, including automatic recovery from an initial Battle.net
sign-out. A previous candidate had cancelled an acknowledged slow countdown
after 30 seconds; that failure remains recorded separately. The corrected
deadline has a failing-before/passing-after regression based on its trace.

A public lobby on the ranked map then received two human arrivals. Recorded
welcome submission latency was 7 ms and less than 1 ms after detection; the
participating tester confirmed prompt display in Warcraft. These two samples
cover the standard welcome without statistics, not a delivery percentile or a
network round-trip measurement. An independent synthetic statistics lookup
took 1,890 ms for its first request and 176/160 ms for the next two.

The same corrected release executables passed an isolated native Windows
supervisor test: idle startup, controller replacement, supervisor adoption,
duplicate rejection, frozen-controller recovery, durable Quit after a crash,
explicit restart and normal Quit. The test took 153 seconds and left no test
process running. It did not launch Warcraft or modify the hosting profile.

The 600-second startup budget deliberately
allows slow authentication; it is not a promise that every boot finishes within
600 seconds. The reason for that native sign-out remains unknown. Recovery was
observed; a permanently revoked login may still require the account owner to
sign in again.

Infrastructure tests do not replace a four-human ranked match or demonstrate final score submission.
Protected credential access and isolated test-session registration were checked
separately. Keep diagnostic trial tasks separate from the resident supervisor
and remove their startup triggers before public hosting.

## Capacity observed

The accepted idle observation lasted 1,226 seconds, with 40 samples, no unhealthy
worker and the same game lifetime and lobby throughout. On the tested two-CPU,
4 GiB Windows Server 2022 instance, total CPU averaged 63.7% and peaked at 98.3%.
Available physical memory stayed at or above 662 MiB. Battle.net averaged about
0.28 CPU cores and Warcraft 0.62; the GHost processes together averaged 0.067
cores with a peak combined private allocation of 7 MiB. Warcraft's paused browser
used no measurable CPU during this interval. These are measurements of this
deployment, including its diagnostic workload, not hardware requirements or a
guarantee of spare capacity.

An earlier observation on the same server averaged 84.4% CPU. The measurements
do not isolate a cause for that variation; retain capacity margin for startup,
game updates and bursts of activity.

Disabling Battle.net browser hardware acceleration was also tried privately.
Short samples still showed high total CPU and the trial's first native start
did not reach loading before cancellation. No cause was established from that
single failure. The setting was restored; it is not part of this preset.
Do not replace Blizzard graphics libraries or disable driver-signing checks to
reproduce this deployment.

## Retain a recoverable deployment

Keep the host ZIP and its checksum, the selected map and its checksum, and the
data directory. Profiles, the next lobby number, hosting intent and reporting
queue belong to the data directory and must survive an executable upgrade.
Protect that directory separately; it is not part of a distributable bundle.

The accompanying private source snapshot contains a file manifest, the exact
Rust toolchain version and `BUILD-SOURCE.ps1`. Verify it with
`BUILD-SOURCE.ps1 -VerifyOnly`; run the same script without that switch to rebuild
the two host executables using the recorded MSVC toolchain and Windows SDK.
The archived sources were restored and successfully compiled, and the rebuilt
CLI was checked. Byte-identical executable rebuilding is not claimed.

The separate operations archive records the private acceptance procedure and
script hashes. It contains no live profiles or credentials. Its order is:
protected profile, computer-only rehost test, cold reboot, stable ranked lobby,
controller crash, twenty-minute observation, test cleanup, then public hosting.
Retain failed receipts as failures. A new executable, game build or server image
requires fresh native evidence rather than reusing an old successful receipt.
