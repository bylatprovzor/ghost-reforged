use super::{Event, status};
use crate::{
    background,
    config::Config,
    discovery::{self, DiscoveryError},
    engine::Phase,
    host_identity::HostIdentity,
    protocol::Lobby,
    transport::Client,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct Connection {
    pub initialization_required: bool,
    pub client: Client,
    pub pending: VecDeque<super::pending::Callback>,
}

#[derive(Default)]
pub(super) struct Initialization {
    pending: bool,
}

impl Initialization {
    fn observe_discovery(&mut self, discovery: &Result<discovery::Endpoint, DiscoveryError>) {
        // A listener may appear before the first WebSocket handshake succeeds.
        // Preserve native startup ownership until a Connection reaches the engine.
        self.pending |= needs_initialization(discovery);
    }

    fn connected(&mut self) -> bool {
        std::mem::take(&mut self.pending)
    }
}

#[derive(Default)]
pub(super) struct SessionWait {
    phase: Option<(Phase, Instant)>,
    last_reply: Option<Instant>,
    signed_out: Option<Instant>,
}

impl SessionWait {
    pub fn observe(&mut self, event: &Value, now: Instant) {
        if event["messageType"].as_str().is_some() {
            self.last_reply = Some(now);
        }
        if event["messageType"] == "LoggedOut" {
            self.signed_out.get_or_insert(now);
        } else if event["messageType"] == "UpdateUserInfo"
            && event["payload"]["user"]["isAuthenticated"] == true
        {
            self.signed_out = None;
        }
    }

    pub fn check(
        &mut self,
        phase: &Phase,
        now: Instant,
        budget: Duration,
    ) -> Result<(), &'static str> {
        if self.expired(phase, now, budget) {
            return Err("Waiting for a fresh Battle.net session.");
        }
        Ok(())
    }

    pub fn expired(&mut self, phase: &Phase, now: Instant, budget: Duration) -> bool {
        if !matches!(
            phase,
            Phase::AwaitMenu | Phase::Initializing(_) | Phase::AwaitSignIn | Phase::Recovering(_)
        ) {
            self.phase = None;
            self.last_reply = None;
            self.signed_out = None;
            return false;
        }
        if self
            .phase
            .as_ref()
            .is_none_or(|(previous, _)| previous != phase)
        {
            self.phase = Some((phase.clone(), now));
        }
        let began = self.phase.as_ref().unwrap().1;
        // Social callbacks can continue after native authentication is lost.
        // They must not extend an explicitly disconnected session indefinitely.
        if self
            .signed_out
            .is_some_and(|lost| now.saturating_duration_since(lost) >= budget)
        {
            return true;
        }
        // A server queue or user prompt can outlive startup. Keep observing while
        // native callbacks answer our read-only polls, but recover a silent API.
        let progress = if *phase == Phase::AwaitSignIn {
            self.last_reply.map_or(began, |reply| reply.max(began))
        } else {
            began
        };
        now.saturating_duration_since(progress) >= budget
    }
}

pub(super) fn open(
    config: &Config,
    tx: &Sender<Event>,
    stop: &AtomicBool,
    repair: &mut super::repair::Repair,
    resources: &mut super::supervision::Resources,
) -> Result<Option<Connection>, String> {
    loop {
        let opened = open_once(config, tx, stop, repair, resources);
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let Some(mut connection) = opened? else {
            return Ok(None);
        };
        let candidate = connection
            .pending
            .iter()
            .rev()
            .find(|event| event["messageType"] == "GameLobbySetup")
            .and_then(|event| serde_json::from_value::<Lobby>(event["payload"].clone()).ok())
            .filter(|lobby| repair.repairable(lobby));
        if let Some(candidate) = candidate
            && repair.confirm_with_recovery(
                &candidate,
                &mut connection.client,
                &mut connection.pending,
                tx,
                stop,
                &mut resources.login_recovery,
            )?
        {
            if stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            status(tx, "Failed lobby detected. Restarting Warcraft III…");
            resources
                .game
                .as_mut()
                .ok_or("Client is unavailable")?
                .terminate()?;
            resources.clear_dead();
            repair.forget_creation();
            continue;
        }
        return Ok(Some(connection));
    }
}

fn open_once(
    config: &Config,
    tx: &Sender<Event>,
    stop: &AtomicBool,
    repair: &mut super::repair::Repair,
    resources: &mut super::supervision::Resources,
) -> Result<Option<Connection>, String> {
    status(tx, "Connecting to Warcraft III…");
    let progress = resources.progress.clone();
    progress.pulse(super::health::Stage::Connecting);
    let generation = resources.generation;
    resources.clear_dead();
    let background = &mut resources.game;
    let launcher = &mut resources.launcher;
    let startup = &mut resources.startup;
    let startup_budget = Duration::from_secs(config.startup_timeout_seconds);
    let mut deadline = Instant::now() + startup_budget;
    let discovery = discovery::discover();
    resources.initialization.observe_discovery(&discovery);
    let mut launch_requested = false;
    launcher.watch(stop, deadline)?;
    let endpoint = match discovery {
        Ok(endpoint) => endpoint,
        Err(DiscoveryError::NotRunning | DiscoveryError::NoEndpoint) => {
            if matches!(discovery, Err(DiscoveryError::NotRunning)) && background.is_none() {
                status(tx, "Starting Warcraft III through Battle.net…");
                *background = launch(launcher, startup, stop, deadline)?;
                deadline = Instant::now() + startup_budget;
                launch_requested = true;
            }
            loop {
                progress.pulse(super::health::Stage::Connecting);
                if stop.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                launcher.poll(stop, deadline)?;
                if background.is_none()
                    && let Ok(pid) = discovery::process_id()
                {
                    *background = Some(background::Background::attach(pid)?);
                }
                if let Some(game) = background.as_mut() {
                    game.set_minimize_window(config.minimize_window);
                    launcher.client_observed();
                    startup.observe(game.pid());
                    if discovery::has_listener(game.pid()).unwrap_or(false) {
                        resources.login_recovery.listener_ready(startup, game.pid());
                    }
                    super::priority::connecting(game)?;
                    game.hide_until(stop, deadline)?;
                }
                if stop.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                let discovered = discovery::discover();
                if background.as_ref().is_some_and(|game| !game.running()) {
                    resources.identity = None;
                    *background = None;
                    startup.ready();
                    resources.login_recovery = Default::default();
                    resources.generation = resources.generation.wrapping_add(1);
                }
                if background.is_none() && request_launch(&discovered, &mut launch_requested) {
                    // An endpoint-less client can exit between discovery attempts.
                    drop(background.take());
                    status(tx, "Starting Warcraft III through Battle.net…");
                    *background = launch(launcher, startup, stop, deadline)?;
                    deadline = Instant::now() + startup_budget;
                }
                match discovered {
                    Ok(endpoint)
                        if background
                            .as_ref()
                            .is_none_or(|game| endpoint.pid == game.pid()) =>
                    {
                        break endpoint;
                    }
                    Ok(_) | Err(DiscoveryError::Ambiguous) => {
                        return Err("Another Warcraft III client appeared.".into());
                    }
                    Err(DiscoveryError::NotRunning | DiscoveryError::NoEndpoint) => {}
                    Err(error) => return Err(error.to_string()),
                }
                if Instant::now() >= deadline {
                    return Err("Warcraft III did not become available yet.".into());
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        Err(e) => return Err(e.to_string()),
    };
    resources
        .login_recovery
        .listener_ready(startup, endpoint.pid);
    launcher.client_observed();
    if endpoint.version.as_deref() != Some(config.expected_version.as_str()) {
        return Err("This Warcraft III build has not been verified yet.".into());
    }
    if background
        .as_ref()
        .is_some_and(|game| game.pid() != endpoint.pid)
    {
        return Err("Another Warcraft III client appeared.".into());
    }
    if background.is_none() {
        *background = Some(background::Background::attach(endpoint.pid)?);
    }
    let game = background.as_mut().ok_or("Client is unavailable")?;
    game.set_minimize_window(config.minimize_window);
    super::priority::connecting(game)?;
    game.hide_until(stop, deadline)?;
    resources.login_recovery.connection_changed();
    let mut client = Client::connect(&endpoint).map_err(|e| e.to_string())?;
    if resources.identity.is_none() {
        resources.identity = Some(HostIdentity::open(endpoint.pid)?);
    }
    client
        .send("SendGameLobbySetup", json!({}))
        .map_err(|e| e.to_string())?;
    client
        .send("SendGameListing", json!({}))
        .map_err(|e| e.to_string())?;
    client
        .send("GetGameList", json!({}))
        .map_err(|e| e.to_string())?;
    client
        .send("GetAuthInfo", json!({}))
        .map_err(|e| e.to_string())?;
    let Some(mut pending) =
        initial_callbacks(&mut client, repair, &mut resources.login_recovery, stop)?
    else {
        return Ok(None);
    };
    if resources.generation != generation {
        repair.forget_creation();
    }
    let screen = pending
        .iter()
        .rev()
        .find(|event| event["messageType"] == "SetGlueScreen")
        .and_then(|event| event["payload"]["screen"].as_str())
        .map(str::to_owned);
    if matches!(screen.as_deref(), Some("SCORE_SCREEN" | "LOADING_SCREEN")) {
        pending.retain(|event| event["messageType"] == "SetGlueScreen");
    } else {
        for event in &pending {
            if event["messageType"] == "GameLobbySetup" {
                let value: Lobby = serde_json::from_value(event["payload"].clone())
                    .map_err(|_| "The native lobby format changed.")?;
                if !crate::recovery::incomplete_lobby(&value)
                    && !repair.rejected_lobby(&value)
                    && (value.local().is_some() || value.is_host)
                    && crate::recovery::resumable_lobby(config, &value).is_none()
                {
                    return Err("This lobby belongs to another session. Leave it before hosting with this profile.".into());
                }
            }
        }
        // A resumable roster must reach the engine before any listing can create another lobby.
        prioritize_adoption(&mut pending);
    }
    if stop.load(Ordering::SeqCst) {
        return Ok(None);
    }
    launcher.complete();
    Ok(Some(Connection {
        initialization_required: resources.initialization.connected(),
        client,
        pending,
    }))
}

fn launch(
    launcher: &mut background::LauncherLaunch,
    startup: &mut crate::background_startup::Startup,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<Option<background::Background>, String> {
    if launcher.pending() {
        return Ok(None);
    }
    crate::background_prerequisites::check()?;
    launcher.prepare_start(stop, deadline)?;
    if stop.load(Ordering::SeqCst) {
        return Ok(None);
    }
    // The user or launcher may have opened a client during the warmup.
    match discovery::process_id() {
        Ok(pid) => {
            launcher.client_observed();
            return background::Background::attach(pid).map(Some);
        }
        Err(DiscoveryError::NotRunning) => {}
        Err(error) => return Err(error.to_string()),
    }
    startup.requested();
    let result = background::launch(launcher, stop, deadline);
    if result.is_err() || stop.load(Ordering::SeqCst) {
        startup.ready();
    }
    result
}

fn initial_callbacks(
    client: &mut Client,
    repair: &mut super::repair::Repair,
    recovery: &mut super::login_recovery::Recovery,
    stop: &AtomicBool,
) -> Result<Option<VecDeque<super::pending::Callback>>, String> {
    let mut pending = VecDeque::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if let Some(event) = client.receive().map_err(|e| e.to_string())? {
            let mut event = super::pending::Callback::from(event);
            repair.inspect_pending(&mut event);
            recovery.observe_pending(&mut event, Instant::now());
            pending.push_back(event);
        }
    }
    Ok(Some(pending))
}

fn prioritize_adoption(pending: &mut VecDeque<super::pending::Callback>) {
    if let Some(index) = pending
        .iter()
        .position(|event| event["messageType"] == "GameLobbySetup")
    {
        // Repair must see the chronological menu/placeholder sequence. Only a
        // populated roster can authorize adoption ahead of an older listing.
        if serde_json::from_value::<Lobby>(pending[index]["payload"].clone())
            .is_ok_and(|lobby| crate::recovery::incomplete_lobby(&lobby))
        {
            return;
        }
        let first = pending
            .remove(index)
            .expect("index came from the same queue");
        pending.push_front(first);
    }
}

fn needs_initialization(discovery: &Result<discovery::Endpoint, DiscoveryError>) -> bool {
    matches!(
        discovery,
        Err(DiscoveryError::NotRunning | DiscoveryError::NoEndpoint)
    )
}

fn request_launch(
    discovered: &Result<discovery::Endpoint, DiscoveryError>,
    requested: &mut bool,
) -> bool {
    if !*requested && matches!(discovered, Err(DiscoveryError::NotRunning)) {
        *requested = true;
        true
    } else {
        false
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
