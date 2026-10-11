use crate::{engine::Engine, protocol::Lobby};

#[derive(Debug, Clone, PartialEq)]
pub enum Activity {
    LobbyCreated(String),
    Joined(String),
    Left(String),
    GameStarted(String),
    Command(String),
}

#[derive(Clone)]
struct Participant {
    id: Option<i64>,
    name: String,
}

impl Participant {
    fn same(&self, other: &Self) -> bool {
        match (self.id, other.id) {
            (Some(left), Some(right)) if left >= 0 && right >= 0 => {
                left == right && self.name == other.name
            }
            _ => self.name == other.name,
        }
    }
}

#[derive(Default)]
pub(super) struct ActivityTracker {
    lobby: Option<String>,
    map: String,
    players: Vec<Participant>,
    started: bool,
}

impl ActivityTracker {
    pub fn observe(&mut self, engine: &Engine, now: u64, roster_changed: bool) -> Vec<Activity> {
        let mut events = Vec::new();
        if roster_changed && let Some(lobby) = engine.owned_lobby(now) {
            if self.lobby.as_deref() != Some(&lobby.lobby_name) {
                self.lobby = Some(lobby.lobby_name.clone());
                self.map = lobby.map_data.map_path.clone();
                self.players.clear();
                self.started = false;
                events.push(Activity::LobbyCreated(lobby.lobby_name.clone()));
            }
            events.extend(self.roster(lobby));
        }
        if engine.game_started()
            && !self.started
            && let Some(name) = &self.lobby
        {
            self.started = true;
            events.push(Activity::GameStarted(name.clone()));
        }
        events
    }

    fn roster(&mut self, lobby: &Lobby) -> Vec<Activity> {
        let players: Vec<_> = lobby
            .players
            .iter()
            .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
            .filter_map(|p| {
                // Native join updates can reserve a slot before resolving its name.
                // Keep a known identity through that transient state, and wait for
                // a name before publishing a newly occupied slot.
                if p.name.trim().is_empty() {
                    return p.id.filter(|id| *id >= 0).and_then(|id| {
                        self.players.iter().find(|old| old.id == Some(id)).cloned()
                    });
                }
                Some(Participant {
                    id: p.id,
                    name: p.name.clone(),
                })
            })
            .collect();
        let mut events = Vec::new();
        for old in &self.players {
            if !players.iter().any(|new| old.same(new)) {
                events.push(Activity::Left(old.name.clone()));
            }
        }
        for new in &players {
            if !self.players.iter().any(|old| old.same(new)) {
                events.push(Activity::Joined(new.name.clone()));
            }
        }
        self.players = players;
        events
    }

    pub fn observed_roster(&mut self, value: &serde_json::Value) -> Vec<Activity> {
        let Ok(lobby) = serde_json::from_value::<Lobby>(value.clone()) else {
            return Vec::new();
        };
        if !lobby.is_host
            || lobby.local().is_none()
            || self.lobby.as_deref() != Some(&lobby.lobby_name)
            || !crate::protocol::same_map_path(&self.map, &lobby.map_data.map_path)
        {
            return Vec::new();
        }
        self.roster(&lobby)
    }
}
