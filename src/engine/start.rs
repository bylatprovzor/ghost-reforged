use super::{Engine, Lobby, OBSERVER_TEAM, Phase};
use crate::protocol::Player;

pub(super) struct StartRoster {
    players: Vec<Participant>,
    pub manual: bool,
    pub cancel_requested: bool,
    acknowledged: Option<u64>,
    cancel_since: Option<u64>,
}

struct Participant {
    slot: u8,
    id: Option<i64>,
    name: String,
    kind: u8,
}

fn playing(player: &Player) -> bool {
    player.slot_status == 2 && !player.is_observer && player.team != OBSERVER_TEAM
}

impl StartRoster {
    fn new(lobby: &Lobby, manual: bool) -> Self {
        Self {
            players: lobby
                .players
                .iter()
                .filter(|p| playing(p))
                .map(|p| Participant {
                    slot: p.slot,
                    id: p.id.filter(|id| *id >= 0),
                    name: p.name.clone(),
                    kind: p.slot_type,
                })
                .collect(),
            manual,
            cancel_requested: false,
            acknowledged: None,
            cancel_since: None,
        }
    }

    pub fn acknowledge(&mut self, now: u64) {
        self.acknowledged.get_or_insert(now);
    }

    pub fn cancel(&mut self, now: u64) {
        self.cancel_requested = true;
        self.cancel_since.get_or_insert(now);
    }

    pub fn intact(&self, lobby: &Lobby) -> bool {
        self.changed(lobby).is_none()
    }

    fn changed(&self, lobby: &Lobby) -> Option<&Participant> {
        self.players.iter().find(|old| {
            !lobby.players.iter().any(|p| {
                playing(p)
                    && p.slot == old.slot
                    && p.slot_type == old.kind
                    && p.id.filter(|id| *id >= 0) == old.id
                    // Native profiles can resolve after the slot is occupied.
                    // A stable player ID survives that temporary unnamed state.
                    && (p.name == old.name
                        || old.id.is_some() && (old.name.is_empty() || p.name.is_empty()))
            })
        })
    }

    pub fn change(&self, lobby: &Lobby) -> Option<String> {
        let old = self.changed(lobby)?;
        let current = lobby.players.iter().find(|p| p.slot == old.slot);
        Some(match current {
            Some(p) => format!(
                "playing slot {} changed (status {}, type {}, team {}, identity changed: {})",
                old.slot + 1,
                p.slot_status,
                p.slot_type,
                p.team,
                p.id.filter(|id| *id >= 0) != old.id || p.name != old.name
            ),
            None => format!(
                "playing slot {} disappeared from the native roster",
                old.slot + 1
            ),
        })
    }
}

impl Engine {
    pub(super) fn expired_command_budget(&self, now: u64) -> Option<u64> {
        let command = self.config.command_timeout_seconds;
        let (since, budget) = match self.phase {
            Phase::Starting(sent) => match self.start.as_ref() {
                Some(start) if start.cancel_since.is_some() => (start.cancel_since?, command),
                Some(start) if start.acknowledged.is_some() => (
                    start.acknowledged?,
                    self.config.load_timeout_seconds.max(command),
                ),
                _ => (sent, command),
            },
            Phase::Leaving(sent) => (sent, command),
            _ => return None,
        };
        (now.saturating_sub(since) >= budget).then_some(budget)
    }

    pub(crate) fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
    pub(super) fn begin_start(&mut self, now: u64, manual: bool) {
        self.start = self
            .latest
            .as_ref()
            .map(|(lobby, _)| StartRoster::new(lobby, manual));
        self.cancelled_roster = None;
        self.countdown_locked = false;
        self.start_confirmed = false;
        self.phase = Phase::Starting(now);
    }
}
