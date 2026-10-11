use super::{Engine, Lobby, OBSERVER_TEAM, Phase};
use crate::protocol::Player;

pub(super) struct StartRoster {
    players: Vec<Participant>,
    pub manual: bool,
    pub cancel_requested: bool,
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
        }
    }

    pub fn intact(&self, lobby: &Lobby) -> bool {
        self.players.iter().all(|old| {
            lobby.players.iter().any(|p| {
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
}

impl Engine {
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
