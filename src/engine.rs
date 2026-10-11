//! Single-threaded controller. Socket callbacks cannot issue gameplay commands.
use crate::{
    config::{Config, HostMode},
    protocol::{self, Command, Lobby, OBSERVER_TEAM},
};
use serde_json::{Value, json};
mod lifecycle;
mod recovery;
mod start;
use start::StartRoster;

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    AwaitMenu,
    Initializing(u64),
    AwaitSignIn,
    Creating(u64),
    Lobby,
    Countdown(u64),
    Starting(u64),
    InGame,
    WatchingGame,
    Restarting,
    Releasing(u64),
    Leaving(u64),
    Cooldown(u64),
    Recovering(u64),
    Stopped(String),
}

pub struct Engine {
    pub config: Config,
    pub phase: Phase,
    number: u32,
    map_index: usize,
    completed: u32,
    owned: bool,
    creation: protocol::CreationEvidence,
    latest: Option<(Lobby, u64)>,
    empty_since: Option<u64>,
    move_sent: Option<u64>,
    provider_ready: bool,
    start: Option<StartRoster>,
    countdown_locked: bool,
    start_confirmed: bool,
    menu_required: bool,
    initialization_required: bool,
    cancelled_roster: Option<String>,
    recovery_cancel: Option<u64>,
    retry_start_after: u64,
    notice: Option<String>,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        let number = config.first_number;
        Self {
            config,
            phase: Phase::AwaitMenu,
            number,
            map_index: 0,
            completed: 0,
            owned: false,
            creation: protocol::CreationEvidence::default(),
            latest: None,
            empty_since: None,
            move_sent: None,
            provider_ready: false,
            start: None,
            countdown_locked: false,
            start_confirmed: false,
            menu_required: false,
            initialization_required: false,
            cancelled_roster: None,
            recovery_cancel: None,
            retry_start_after: 0,
            notice: None,
        }
    }

    pub fn stop(&mut self, reason: &str) {
        self.phase = Phase::Stopped(reason.to_owned());
    }

    pub(crate) fn shutdown_identity(&self) -> Option<(String, String)> {
        matches!(
            self.phase,
            Phase::Creating(_)
                | Phase::Lobby
                | Phase::Countdown(_)
                | Phase::Starting(_)
                | Phase::Recovering(_)
        )
        .then(|| {
            (
                self.config.name(self.number),
                protocol::client_path(&self.config.maps[self.map_index]),
            )
        })
    }

    pub fn update_start_rule(&mut self, minimum: u8, full: bool) {
        if self.config.min_players != minimum || self.config.require_full != full {
            self.cancelled_roster = None;
            self.config.min_players = minimum;
            self.config.require_full = full;
        }
    }

    pub fn update_rehost(&mut self, enabled: bool, delay: u64, now: u64) {
        self.config.leave_on_load = enabled;
        self.config.restart_on_load = enabled;
        self.config.max_games = if enabled {
            0
        } else {
            self.completed.saturating_add(1)
        };
        if let Phase::Cooldown(until) = &mut self.phase {
            let began = until.saturating_sub(self.config.rehost_delay_seconds);
            *until = began.saturating_add(delay).max(now);
            if !enabled {
                self.stop("Configured game limit reached");
            }
        }
        self.config.rehost_delay_seconds = delay;
    }

    pub fn on_event(&mut self, event: &Value, now: u64) -> Vec<Command> {
        if matches!(self.phase, Phase::Stopped(_)) {
            return Vec::new();
        }
        if matches!(self.phase, Phase::Recovering(_)) {
            return self.reconcile(event, now);
        }
        let kind = event["messageType"].as_str().unwrap_or("");
        let payload = &event["payload"];
        if !self.owned
            && (self.awaiting_session() || self.phase == Phase::WatchingGame)
            && kind == "SetGlueScreen"
            && payload["screen"] == "SCORE_SCREEN"
        {
            self.phase = Phase::AwaitMenu;
            self.menu_required = true;
            self.provider_ready = false;
            return vec![
                Command::empty("ScoreScreenClose"),
                Command::empty("InitializeNetProvider"),
            ];
        }
        if !self.owned && kind == "SetGlueScreen" && payload["screen"] == "LOADING_SCREEN" {
            self.phase = Phase::WatchingGame;
            return Vec::new();
        }
        if kind == "OnNetProviderInitialized" && protocol::bnet_confirmation(event) == Some(true) {
            self.initialization_required = false;
        }
        if kind == "SetGlueScreen"
            && matches!(
                payload["screen"].as_str(),
                Some("MAIN_MENU" | "MULTIPLAYER_CUSTOM_GAMES_SETUP" | "CUSTOM_LOBBIES")
            )
        {
            self.menu_required = false;
            if self.phase == Phase::WatchingGame {
                self.phase = Phase::AwaitMenu;
            }
        }
        if matches!(self.phase, Phase::AwaitMenu | Phase::Cooldown(_))
            && let Some(ready) = protocol::bnet_confirmation(event)
        {
            self.provider_ready = ready && !self.initialization_required;
        }
        if matches!(self.phase, Phase::Creating(_)) {
            if let Err(error) = self.creation.observe(event) {
                self.stop(&error.to_string());
                return Vec::new();
            }
            if self.creation.confirmed()
                && matches!(kind, "MultiplayerGameCreateResult" | "SetGlueScreen")
            {
                return vec![Command::empty("SendGameLobbySetup")];
            }
        }
        match kind {
            "LoggedOut" => {
                self.provider_ready = false;
                self.menu_required = true;
                if !(matches!(self.phase, Phase::InGame | Phase::Releasing(_))
                    || matches!(self.phase, Phase::Starting(_)) && self.countdown_locked)
                {
                    self.wait_for_sign_in(now);
                }
            }
            "GameList" if self.awaiting_session() && !self.menu_required => {
                if self.initialization_required {
                    if self.phase == Phase::AwaitMenu {
                        self.phase = Phase::Initializing(now);
                        return vec![Command::empty("InitializeNetProvider")];
                    }
                    return Vec::new();
                }
                if self.config.host_mode == HostMode::Invisible {
                    self.stop("Invisible hosting remains unvalidated; no lobby created");
                } else {
                    if self.provider_ready || protocol::has_online_listing(payload) {
                        self.provider_ready = true;
                        return self.create(now);
                    }
                    if self.phase == Phase::AwaitMenu {
                        self.phase = Phase::Initializing(now);
                        return vec![Command::empty("InitializeNetProvider")];
                    }
                }
            }
            "OnNetProviderChanged" | "OnNetProviderInitialized"
                if matches!(self.phase, Phase::Initializing(_) | Phase::AwaitSignIn) =>
            {
                if protocol::bnet_confirmation(event) == Some(true) {
                    if self.initialization_required {
                        return Vec::new();
                    }
                    self.provider_ready = true;
                    if !self.menu_required {
                        return self.create(now);
                    }
                } else {
                    self.provider_ready = false;
                    self.phase = Phase::AwaitSignIn;
                }
            }
            "GameLobbySetup" => {
                let Ok(lobby) = serde_json::from_value::<Lobby>(payload.clone()) else {
                    self.stop("Lobby schema changed; stopped before issuing further commands");
                    return Vec::new();
                };
                let matches_name = lobby.lobby_name == self.config.name(self.number);
                let map = protocol::client_path(&self.config.maps[self.map_index]);
                let matches_map = protocol::same_map_path(&lobby.map_data.map_path, &map);
                if !self.owned
                    && self.awaiting_session()
                    && let Some((number, map_index)) =
                        crate::recovery::resumable_lobby(&self.config, &lobby)
                {
                    self.number = number;
                    self.map_index = map_index;
                    self.owned = true;
                    self.latest = Some((lobby, now));
                    self.phase = Phase::Lobby;
                    return Vec::new();
                }
                if matches!(self.phase, Phase::Creating(_))
                    && matches_name
                    && matches_map
                    && lobby.is_host
                    && lobby
                        .local()
                        .is_some_and(|p| p.slot_status == 2 && p.slot_type == 0)
                    && self.creation.confirmed()
                {
                    self.owned = true;
                    self.phase = Phase::Lobby;
                } else if self.owned
                    && (!matches_name || !matches_map || !lobby.is_host)
                    && !matches!(
                        self.phase,
                        Phase::InGame | Phase::Releasing(_) | Phase::Leaving(_)
                    )
                {
                    self.stop("Lobby identity or host authority changed; no further commands sent");
                    return Vec::new();
                } else if !self.owned
                    && !matches!(self.phase, Phase::Creating(_))
                    && (lobby.is_host || lobby.local().is_some())
                {
                    self.stop("An existing lobby is active; it was not adopted or modified");
                    return Vec::new();
                }
                if self.owned && matches_name && matches_map && lobby.is_host {
                    if self.latest.as_ref().is_some_and(|(previous, _)| {
                        previous.start_signature() != lobby.start_signature()
                    }) {
                        self.retry_start_after = now;
                    }
                    if lobby.empty() {
                        self.empty_since.get_or_insert(now);
                    } else {
                        self.empty_since = None;
                    }
                    self.latest = Some((lobby, now));
                }
            }
            "LobbyCountdown" if self.owned => {
                if payload["details"]["countdownActive"] == true {
                    if matches!(self.phase, Phase::Lobby | Phase::Countdown(_)) {
                        self.begin_start(now, true);
                    }
                    self.countdown_locked |= payload["details"]["countdownLocked"] == true;
                }
                if payload["details"]["countdownActive"] == false
                    && matches!(self.phase, Phase::Starting(_))
                {
                    // LobbyCancel is accepted even after the native UI locks.
                    // Its inactive callback is an acknowledgement, not a start.
                    let cancelled = self.start.as_ref().is_some_and(|s| s.cancel_requested);
                    self.start_confirmed |= self.countdown_locked && !cancelled;
                    if !self.countdown_locked || cancelled {
                        self.cancelled_roster = if cancelled {
                            self.latest
                                .as_ref()
                                .map(|(lobby, _)| lobby.start_signature())
                        } else {
                            None
                        };
                        self.retry_start_after = if cancelled {
                            now
                        } else {
                            now.saturating_add(5)
                        };
                        self.start = None;
                        self.countdown_locked = false;
                        self.phase = Phase::Lobby;
                    } else if self.config.leave_on_load
                        && self.config.host_mode == HostMode::Observer
                        && !self.config.restart_on_load
                    {
                        self.phase = Phase::Releasing(now);
                        return vec![Command::empty("LeaveGame")];
                    }
                }
            }
            "SetGlueScreen" if self.owned => match payload["screen"].as_str() {
                Some("LOADING_SCREEN")
                    if matches!(self.phase, Phase::Starting(_))
                        || (self.cancelled_roster.is_some()
                            && matches!(self.phase, Phase::Lobby | Phase::Countdown(_))) =>
                {
                    // Loading remains authoritative if cancellation raced with
                    // the final native transition instead of being accepted.
                    self.start_confirmed = true;
                    if self.config.leave_on_load
                        && self.config.host_mode == HostMode::Observer
                        && self
                            .latest
                            .as_ref()
                            .and_then(|(lobby, _)| lobby.local())
                            .is_some_and(|p| p.is_observer)
                    {
                        if self.config.restart_on_load {
                            self.phase = Phase::Restarting;
                            return Vec::new();
                        }
                        self.phase = Phase::Releasing(now);
                        return vec![Command::empty("LeaveGame")];
                    }
                    self.phase = Phase::InGame;
                }
                Some("SCORE_SCREEN") => {
                    self.finish_game(now);
                    return vec![
                        Command::empty("ScoreScreenClose"),
                        Command::empty("InitializeNetProvider"),
                    ];
                }
                Some("MULTIPLAYER_CUSTOM_GAMES_SETUP" | "CUSTOM_LOBBIES" | "MAIN_MENU")
                    if matches!(self.phase, Phase::InGame | Phase::Releasing(_)) =>
                {
                    self.finish_game(now)
                }
                _ => {}
            },
            "MultiplayerGameLeave"
                if self.owned
                    && !matches!(
                        self.phase,
                        Phase::InGame | Phase::Releasing(_) | Phase::Starting(_)
                    ) =>
            {
                self.release(now);
            }
            _ => {}
        }
        self.tick(now)
    }

    fn create(&mut self, now: u64) -> Vec<Command> {
        self.phase = Phase::Creating(now);
        self.creation = protocol::CreationEvidence::default();
        vec![protocol::create_lobby(
            &self.config,
            self.number,
            self.map_index,
        )]
    }

    pub fn number(&self) -> u32 {
        self.number
    }

    pub(crate) fn game_started(&self) -> bool {
        self.owned && self.start_confirmed
    }

    pub fn owned_lobby(&self, now: u64) -> Option<&Lobby> {
        let (lobby, seen) = self.latest.as_ref()?;
        (self.owned
            && lobby.is_host
            && matches!(
                self.phase,
                Phase::Lobby | Phase::Countdown(_) | Phase::Starting(_)
            )
            && now.saturating_sub(*seen) < self.config.stale_after_seconds)
            .then_some(lobby)
    }

    pub fn can_start(&self, now: u64) -> bool {
        matches!(self.phase, Phase::Lobby | Phase::Countdown(_))
            && self.owned_lobby(now).is_some_and(|lobby| {
                lobby
                    .local()
                    .is_some_and(|p| self.config.host_mode != HostMode::Observer || p.is_observer)
                    && lobby.occupied_players() > 0
            })
    }

    pub fn start_now(&mut self, now: u64) -> Vec<Command> {
        if self.can_start(now) {
            self.begin_start(now, true);
            vec![Command::empty("LobbyStart")]
        } else {
            Vec::new()
        }
    }

    pub fn slot_command(&self, slot: u8, open: bool, now: u64) -> Option<Command> {
        let lobby = self.owned_lobby(now)?;
        if !matches!(self.phase, Phase::Lobby) {
            return None;
        }
        let player = lobby.players.iter().find(|p| p.slot == slot)?;
        if player.is_self || player.slot_status != u8::from(open) {
            return None;
        }
        Some(Command::new(
            if open { "OpenSlot" } else { "CloseSlot" },
            json!({"slot":slot}),
        ))
    }

    pub fn tick(&mut self, now: u64) -> Vec<Command> {
        if let Phase::Initializing(sent) = self.phase
            && now.saturating_sub(sent) >= self.config.command_timeout_seconds
        {
            self.phase = Phase::AwaitSignIn;
            return Vec::new();
        }
        match self.phase {
            Phase::Releasing(sent)
                if now.saturating_sub(sent) >= self.config.load_timeout_seconds =>
            {
                self.stop("Observer departure was not confirmed before the loading timeout");
                return Vec::new();
            }
            Phase::Initializing(sent)
            | Phase::Creating(sent)
            | Phase::Starting(sent)
            | Phase::Leaving(sent)
                if now.saturating_sub(sent) >= self.config.command_timeout_seconds =>
            {
                self.notice = Some(format!(
                    "Native command timed out in {:?} after {} s; reconciling the lobby.",
                    self.phase, self.config.command_timeout_seconds
                ));
                self.recover(now);
                return vec![Command::empty("SendGameLobbySetup")];
            }
            Phase::Cooldown(until) if now >= until => self.phase = Phase::AwaitMenu,
            _ => {}
        }
        if !matches!(
            self.phase,
            Phase::Lobby | Phase::Countdown(_) | Phase::Starting(_)
        ) || !self.owned
        {
            return Vec::new();
        }
        let Some((lobby, seen)) = &self.latest else {
            return Vec::new();
        };
        if now.saturating_sub(*seen) >= self.config.stale_after_seconds {
            if matches!(self.phase, Phase::Starting(_)) {
                // A silent UI during the native countdown/loading transition
                // is not evidence that a player left. The command deadline
                // above handles a genuinely stalled start.
                return Vec::new();
            }
            if now.saturating_sub(*seen) >= self.config.command_timeout_seconds {
                self.recover(now);
                return vec![Command::empty("SendGameLobbySetup")];
            }
            self.phase = Phase::Lobby;
            return Vec::new();
        }
        if self
            .start
            .as_ref()
            .is_some_and(|start| start.cancel_requested)
        {
            return Vec::new();
        }
        let Some(local) = lobby.local() else {
            return Vec::new();
        };
        if self.config.host_mode == HostMode::Observer && !local.is_observer {
            if !local.team_change_enabled {
                self.stop("The map does not allow moving the host to observers");
                return Vec::new();
            }
            if let Some(sent) = self.move_sent {
                if now.saturating_sub(sent) >= self.config.command_timeout_seconds {
                    self.stop("Observer placement was not confirmed");
                }
                return Vec::new();
            }
            self.move_sent = Some(now);
            return vec![Command::new(
                "SetTeam",
                json!({"slot":local.slot,"team":OBSERVER_TEAM}),
            )];
        }
        let enough = self.start.as_ref().is_none_or(|start| start.intact(lobby))
            && (self.start.as_ref().is_some_and(|start| start.manual)
                || if self.config.require_full {
                    lobby.humans(self.config.count_host) > 0 && lobby.full()
                } else {
                    lobby.humans(self.config.count_host) >= self.config.min_players as usize
                });
        if !enough {
            if matches!(self.phase, Phase::Starting(_)) {
                self.notice = Some(format!(
                    "Countdown cancelled: {}.",
                    self.start
                        .as_ref()
                        .and_then(|start| start.change(lobby))
                        .unwrap_or_else(
                            || "the configured start rule is no longer satisfied".into()
                        )
                ));
                if let Some(start) = &mut self.start {
                    start.cancel_requested = true;
                }
                // Remain Starting until Warcraft acknowledges cancellation.
                return vec![Command::empty("LobbyCancel")];
            }
            self.cancelled_roster = None;
            self.phase = Phase::Lobby;
            if let Some(empty_since) = self.empty_since
                && self.config.empty_rehost_seconds > 0
                && now.saturating_sub(empty_since) >= self.config.empty_rehost_seconds
            {
                self.phase = Phase::Leaving(now);
                return vec![Command::empty("LeaveGame")];
            }
        } else if self.config.auto_start
            && now >= self.retry_start_after
            && self.cancelled_roster.as_ref() != Some(&lobby.start_signature())
        {
            match self.phase {
                Phase::Lobby => self.phase = Phase::Countdown(now + self.config.countdown_seconds),
                Phase::Countdown(until) if now >= until => {
                    self.begin_start(now, false);
                    // The game itself performs its authoritative map/readiness validation.
                    return vec![Command::empty("LobbyStart")];
                }
                _ => {}
            }
        }
        Vec::new()
    }

    pub fn wants_snapshot(&self) -> bool {
        matches!(
            self.phase,
            Phase::Creating(_)
                | Phase::Lobby
                | Phase::Countdown(_)
                | Phase::Starting(_)
                | Phase::Recovering(_)
        )
    }

    pub fn awaiting_session(&self) -> bool {
        matches!(
            self.phase,
            Phase::AwaitMenu | Phase::Initializing(_) | Phase::AwaitSignIn
        )
    }

    fn wait_for_sign_in(&mut self, now: u64) {
        if self.owned || matches!(self.phase, Phase::Creating(_)) {
            self.release(now);
            if matches!(self.phase, Phase::Stopped(_)) {
                return;
            }
        }
        self.provider_ready = false;
        self.phase = Phase::AwaitSignIn;
    }
}
