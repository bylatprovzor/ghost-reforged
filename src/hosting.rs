mod activity;
mod control;
use control::{Reservation, dispatch, reserve};
mod connection;
mod settings;
mod shutdown;
use crate::{
    alias::Verification,
    background,
    config::Config,
    engine::{Engine, Phase},
    protocol::{Command, Lobby},
};
pub use activity::Activity;
use serde_json::{Value, json};
pub use settings::LiveSettings;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "hosting_tests.rs"]
mod tests;

#[derive(Debug, Clone)]
pub enum Control {
    Stop,
    Start,
    StartRule { minimum: u8, full: bool },
    Settings(LiveSettings),
    HostName(String),
    Slot { slot: u8, open: bool },
    Reserved { number: u32, saved: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub name: String,
    pub number: u8,
    pub status: u8,
    pub computer: bool,
    pub observer: bool,
    pub own: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub slots: Vec<Slot>,
    pub humans: usize,
    pub can_start: bool,
    pub can_edit_slots: bool,
}

impl Snapshot {
    pub fn player_capacity(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| !slot.own && !slot.observer && !slot.computer && slot.status != 1)
            .count()
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Activity(Activity),
    ClientChanged(Option<u32>),
    VisualChanged,
    Status(String),
    Lobby(Snapshot),
    LobbyCleared,
    Reserve(u32),
    Finished(Result<(), String>),
}

pub struct Worker {
    commands: Sender<Control>,
    events: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Worker {
    pub fn start(config: Config, alias: String) -> Result<Self, String> {
        config.validate().map_err(|e| e.to_string())?;
        config.ensure_supported().map_err(|e| e.to_string())?;
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        thread::Builder::new()
            .name("ghost-hosting".into())
            .spawn(move || {
                let result = run(config, alias, &rx, &tx, &cancelled);
                let _ = tx.send(Event::Finished(result));
            })
            .map_err(|_| "The hosting worker could not be started.")?;
        Ok(Self {
            commands,
            events,
            stop,
        })
    }
    pub fn send(&self, command: Control) {
        if matches!(command, Control::Stop) {
            self.stop.store(true, Ordering::SeqCst);
        }
        let _ = self.commands.send(command);
    }
    pub fn receive(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn status(tx: &Sender<Event>, text: &str) {
    let _ = tx.send(Event::Status(text.into()));
}

fn run(
    config: Config,
    mut name: String,
    rx: &Receiver<Control>,
    tx: &Sender<Event>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let _lease = background::HostLease::acquire()?;
    let Some(connection::Connection {
        initialization_required,
        mut client,
        mut alias,
        mut game,
        mut pending,
    }) = connection::open(&config, name.clone(), tx, stop)?
    else {
        return Ok(());
    };
    let mut engine = Engine::new(config);
    if initialization_required {
        engine.expect_client_initialization();
    }
    let started = Instant::now();
    let mut requested = Instant::now();
    let mut reported = String::new();
    let mut snapshot = None;
    let mut reserved = None;
    let mut deferred_controls = VecDeque::new();
    let mut alias_ok = false;
    let mut alias_refresh = false;
    let _ = tx.send(Event::ClientChanged(Some(game.pid())));
    let mut visual_state = None;
    let mut activity = activity::ActivityTracker::default();
    let mut chat = crate::commands::LobbyCommands::default();
    let mut announcements = crate::welcome::Announcements::default();
    let mut start_requested = None;
    let mut kick_requested: Option<(crate::commands::KickTarget, u64)> = None;
    loop {
        let now = started.elapsed().as_secs();
        let control = deferred_controls.pop_front().or_else(|| rx.try_recv().ok());
        if stop.load(Ordering::SeqCst) || matches!(control, Some(Control::Stop)) {
            return shutdown::close(&engine, &mut client, &mut pending);
        }
        if let Some(Control::HostName(next)) = &control
            && alias.rename(next)?
        {
            name = next.clone();
            alias_ok = false;
            alias_refresh = true;
            client
                .send("SendGameLobbySetup", json!({}))
                .map_err(|e| e.to_string())?;
        }
        if let Some(Control::Settings(settings)) = &control {
            chat.apply_settings(&settings.commands);
            if !settings
                .commands
                .enabled(crate::commands::ChatCommand::VoteKick)
            {
                kick_requested = None;
            }
        }
        if matches!(control, Some(Control::Start)) {
            start_requested = Some(now);
        }
        let event = if let Some(event) = pending.pop_front() {
            Some(event)
        } else {
            client.receive().map_err(|e| e.to_string())?
        };
        if event
            .as_ref()
            .is_some_and(|e| e["messageType"] == "GameLobbySetup" && !alias.matches(&e["payload"]))
        {
            alias_ok = false;
        }
        let mut commands = dispatch(&mut engine, event.as_ref(), control.as_ref(), alias_ok, now);
        if kick_requested
            .as_ref()
            .is_some_and(|(_, requested)| now.saturating_sub(*requested) >= 5)
            || !matches!(engine.phase, Phase::Lobby | Phase::Countdown(_))
        {
            kick_requested = None;
        }
        if alias_ok
            && event
                .as_ref()
                .is_some_and(|e| e["messageType"] == "GameLobbySetup")
            && let Some((target, _)) = kick_requested.take()
            && let Some(lobby) = engine.owned_lobby(now)
        {
            commands.extend(target.confirm(lobby));
        }
        if let Some(requested) = start_requested {
            if now.saturating_sub(requested) >= 5 || matches!(engine.phase, Phase::Starting(_)) {
                start_requested = None;
            } else if alias_ok && engine.can_start(now) {
                commands.extend(engine.start_now(now));
                start_requested = None;
            }
        }
        if matches!(engine.phase, Phase::Lobby | Phase::Countdown(_)) {
            chat.observe(engine.owned_lobby(now), now);
            if alias_ok && let Some(event) = &event {
                let outcome = chat.handle(event, &engine.config.commands, now);
                if let Some(line) = outcome.activity {
                    let _ = tx.send(Event::Activity(Activity::Command(line)));
                }
                commands.extend(outcome.reply);
                if let Some(target) = outcome.kick {
                    kick_requested = Some((target, now));
                    commands.push(Command::empty("SendGameLobbySetup"));
                }
                if outcome.start {
                    commands.extend(engine.start_now(now));
                }
            }
        } else {
            chat.observe(None, now);
        }
        for event in activity.observe(
            &engine,
            now,
            event
                .as_ref()
                .is_some_and(|event| event["messageType"] == "GameLobbySetup"),
        ) {
            track_announcement(&event, &mut announcements);
            let _ = tx.send(Event::Activity(event));
        }
        if let Some(event) = &event {
            match event["messageType"].as_str() {
                Some("GameLobbySetup") if visual_state.as_ref() != Some(&event["payload"]) => {
                    visual_state = Some(event["payload"].clone());
                    let _ = tx.send(Event::VisualChanged);
                }
                Some("SetGlueScreen" | "LobbyCountdown") => {
                    let _ = tx.send(Event::VisualChanged);
                }
                _ => {}
            }
        }
        publish_lobby(&engine, alias_ok, now, &mut snapshot, tx);
        if engine.phase == Phase::Restarting {
            status(tx, "Restarting Warcraft III…");
            let _ = tx.send(Event::ClientChanged(None));
            game.terminate()?;
            engine.client_stopped(now);
            snapshot = None;
            alias_ok = false;
            let _ = tx.send(Event::LobbyCleared);
            if let Phase::Stopped(reason) = &engine.phase {
                return if reason == "Configured game limit reached" {
                    Ok(())
                } else {
                    Err(reason.clone())
                };
            }
            let Some(next) = connection::open(&engine.config, name.clone(), tx, stop)? else {
                return Ok(());
            };
            if next.initialization_required {
                engine.expect_client_initialization();
            }
            client = next.client;
            alias = next.alias;
            game = next.game;
            let _ = tx.send(Event::ClientChanged(Some(game.pid())));
            visual_state = None;
            pending = next.pending;
            requested = Instant::now();
            reported.clear();
            continue;
        }
        for command in commands {
            if command.message == "CreateLobby" {
                alias.reset();
                alias_ok = false;
                let number = engine.number();
                if reserved != Some(number) {
                    if reserve(number, rx, tx, stop, &mut deferred_controls)?
                        == Reservation::Cancelled
                    {
                        return Ok(());
                    }
                    reserved = Some(number);
                }
            }
            if stop.load(Ordering::SeqCst) {
                // CreateLobby has not been sent yet in this iteration.
                if command.message == "CreateLobby" {
                    return Ok(());
                }
                return shutdown::close(&engine, &mut client, &mut pending);
            }
            client
                .send(command.message, command.payload)
                .map_err(|e| e.to_string())?;
        }
        if let Some(event) = &event
            && event["messageType"] == "GameLobbySetup"
            && engine.owned_lobby(now).is_some()
        {
            let value = &event["payload"];
            if engine
                .owned_lobby(now)
                .unwrap()
                .local()
                .is_some_and(|p| p.is_observer)
            {
                if !alias.matches(value) {
                    status(tx, "Applying host name…");
                    reported.clear();
                    let mut observe = |value: &Value| {
                        for activity in activity.observed_roster(value) {
                            track_announcement(&activity, &mut announcements);
                            let _ = tx.send(Event::Activity(activity));
                        }
                    };
                    let verification = if alias_refresh {
                        alias_refresh = false;
                        alias.refresh_observed(&mut client, value, stop, &mut observe)?
                    } else {
                        alias.ensure_observed(&mut client, value, stop, &mut observe)?
                    };
                    match verification {
                        Verification::Verified {
                            snapshot,
                            pending: observed,
                        }
                        | Verification::Changed {
                            snapshot,
                            pending: observed,
                        } => {
                            pending.extend(observed);
                            pending.push_back(
                                json!({"messageType":"GameLobbySetup","payload":snapshot}),
                            );
                        }
                        Verification::Interrupted(observed) => {
                            pending.extend(observed);
                        }
                    }
                    // Replay native events before accepting more controls or publishing state.
                    continue;
                }
                alias_ok = true;
                game.hide();
            }
        }
        publish_lobby(
            &engine,
            alias_ok,
            started.elapsed().as_secs(),
            &mut snapshot,
            tx,
        );
        if alias_ok
            && matches!(engine.phase, Phase::Lobby | Phase::Countdown(_))
            && let Some(lobby) = engine.owned_lobby(now)
            && let Some(command) = announcements.next(lobby, &engine.config.welcome, now)
        {
            client
                .send(command.message, command.payload)
                .map_err(|e| e.to_string())?;
        }
        if !matches!(engine.phase, Phase::AwaitSignIn | Phase::Stopped(_)) {
            game.hide();
        }
        let phase = phase_label(&engine.phase, now);
        if phase != reported {
            status(tx, &phase);
            reported = phase;
            if engine.phase == Phase::AwaitSignIn {
                return Err("Sign in to Battle.net, open Custom Games, then click Host.".into());
            }
        }
        if let Phase::Stopped(reason) = &engine.phase {
            if reason == "Configured game limit reached" {
                return Ok(());
            }
            return Err(reason.clone());
        }
        if requested.elapsed() >= Duration::from_secs(engine.config.snapshot_interval_seconds) {
            if engine.wants_snapshot() {
                client
                    .send("SendGameLobbySetup", json!({}))
                    .map_err(|e| e.to_string())?;
            } else if engine.awaiting_session() {
                client
                    .send("GetGameList", json!({}))
                    .map_err(|e| e.to_string())?;
            }
            requested = Instant::now();
        }
    }
}

fn track_announcement(event: &Activity, announcements: &mut crate::welcome::Announcements) {
    match event {
        Activity::LobbyCreated(_) | Activity::GameStarted(_) => *announcements = Default::default(),
        Activity::Joined(name) => announcements.joined(name),
        Activity::Left(name) => announcements.left(name),
        Activity::Command(_) => {}
    }
}

fn publish_lobby(
    engine: &Engine,
    alias_ready: bool,
    now: u64,
    previous: &mut Option<Snapshot>,
    tx: &Sender<Event>,
) {
    let current = engine
        .owned_lobby(now)
        .filter(|lobby| alias_ready && lobby.local().is_some_and(|p| p.is_observer))
        .map(|lobby| summarize(lobby, engine.can_start(now), engine.phase == Phase::Lobby));
    if *previous != current {
        let _ = tx.send(match &current {
            Some(lobby) => Event::Lobby(lobby.clone()),
            None => Event::LobbyCleared,
        });
        *previous = current;
    }
}

fn summarize(lobby: &Lobby, can_start: bool, can_edit_slots: bool) -> Snapshot {
    Snapshot {
        name: lobby.lobby_name.clone(),
        humans: lobby.humans(false),
        can_start,
        can_edit_slots,
        slots: lobby
            .players
            .iter()
            .map(|p| Slot {
                name: p.name.clone(),
                number: p.slot,
                status: p.slot_status,
                computer: p.slot_type != 0,
                observer: p.is_observer || p.team == 24,
                own: p.is_self,
            })
            .collect(),
    }
}

fn phase_label(phase: &Phase, now: u64) -> String {
    match phase {
        Phase::AwaitMenu => "Waiting for Battle.net…".into(),
        Phase::Initializing(_) => "Connecting to Battle.net…".into(),
        Phase::AwaitSignIn => "Waiting for Battle.net sign-in…".into(),
        Phase::Creating(_) => "Creating lobby…".into(),
        Phase::Lobby => "Lobby open".into(),
        Phase::Countdown(until) => format!("Starting in {} s", until.saturating_sub(now)),
        Phase::Starting(_) => "Game countdown…".into(),
        Phase::InGame => "Game in progress".into(),
        Phase::WatchingGame => "Waiting for the current game to finish…".into(),
        Phase::Restarting => "Restarting Warcraft III…".into(),
        Phase::Releasing(_) => "Leaving started game…".into(),
        Phase::Leaving(_) => "Closing empty lobby…".into(),
        Phase::Cooldown(until) => format!("Next lobby in {} s", until.saturating_sub(now)),
        Phase::Stopped(reason) => reason.clone(),
    }
}
