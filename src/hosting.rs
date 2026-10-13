mod activity;
mod chat;
mod control;
use control::{Reservation, dispatch, reserve};
mod connection;
mod controller;
mod kick;
mod ladder;
mod repair;
pub use kick::KickRequest;
mod slots;
pub use slots::{SlotAction, SlotRequest};
mod settings;
mod shutdown;
mod supervision;
use crate::{
    background,
    config::Config,
    engine::{Engine, Phase},
    host_identity::HostIdentity,
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
    Slot { slot: u8, open: bool },
    Kick(KickRequest),
    EditSlot(SlotRequest),
    Reserved { number: u32, saved: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub id: Option<i64>,
    pub name: String,
    pub number: u8,
    pub status: u8,
    pub computer: bool,
    pub observer: bool,
    pub own: bool,
    pub change_type: bool,
    pub change_team: bool,
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
    pub fn start(config: Config) -> Result<Self, String> {
        config.validate().map_err(|e| e.to_string())?;
        config.ensure_supported().map_err(|e| e.to_string())?;
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        thread::Builder::new()
            .name("ghost-hosting".into())
            .spawn(move || {
                let result = controller::run(config, &rx, &tx, &cancelled);
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

fn snapshot_interval(engine: &Engine) -> Duration {
    if matches!(engine.phase, Phase::Countdown(_) | Phase::Starting(_)) {
        Duration::from_millis(50)
    } else {
        Duration::from_secs(engine.config.snapshot_interval_seconds)
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
    identity_ready: bool,
    now: u64,
    previous: &mut Option<Snapshot>,
    tx: &Sender<Event>,
) {
    let current = engine
        .owned_lobby(now)
        .filter(|lobby| identity_ready && lobby.local().is_some_and(|p| p.is_observer))
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
                id: p.id,
                name: p.name.clone(),
                number: p.slot,
                status: p.slot_status,
                computer: p.slot_type != 0,
                observer: p.is_observer || p.team == 24,
                own: p.is_self,
                change_type: p.slot_type_change_enabled,
                change_team: p.team_change_enabled,
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
        Phase::Recovering(_) => "Recovering the lobby…".into(),
        Phase::Stopped(reason) => reason.clone(),
    }
}
