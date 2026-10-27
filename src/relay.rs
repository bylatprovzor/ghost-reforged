//! Relay between several hosts on one server (one Windows user and account each).
//!
//! Exactly one host keeps an open lobby. When its game starts, the host that has
//! waited longest at the menu creates the next lobby at once, so players never
//! wait for a rehost. Each host only writes its own file in a shared directory;
//! a host that stops writing is ignored after `STALE_MS`.
//!
//! Enabled by `GHOST_RELAY_DIR` (shared directory) and `GHOST_RELAY_ID`
//! (this host's name, `[A-Za-z0-9_-]{1,32}`).
use crate::engine::{Engine, Phase};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const STALE_MS: u64 = 30_000;
const BEAT_MS: u64 = 5_000;
const READ_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Creating or holding the open lobby.
    Lobby,
    /// Signed in at the menu, waiting for its turn.
    Ready,
    /// In a game, leaving it, signing in or recovering: not in the queue.
    Busy,
    Stopped,
}

impl State {
    pub fn of(engine: &Engine) -> Self {
        if engine.relay_ready() {
            return Self::Ready;
        }
        match engine.phase {
            Phase::Creating(_) | Phase::Lobby | Phase::Countdown(_) | Phase::Recovering(_) => Self::Lobby,
            Phase::Stopped(_) => Self::Stopped,
            _ => Self::Busy,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub state: State,
    /// When the host entered this state (ms since the Unix epoch).
    pub since: u64,
    /// Last write (ms since the Unix epoch).
    pub beat: u64,
}

pub fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `me` must keep waiting. Only live peers count. A peer with the open
/// lobby always wins; among ready hosts the one waiting longest goes first,
/// with the id breaking ties so every host reaches the same answer.
pub fn hold(me: &Peer, peers: &[Peer], now: u64) -> bool {
    peers.iter().filter(|p| p.id != me.id && now.saturating_sub(p.beat) < STALE_MS && p.beat <= now + STALE_MS).any(|p| {
        p.state == State::Lobby
            || (me.state == State::Ready && p.state == State::Ready && (p.since, p.id.as_str()) < (me.since, me.id.as_str()))
    })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub struct Relay {
    dir: PathBuf,
    me: Peer,
    written: u64,
    read: u64,
    held: bool,
    warned: bool,
}

impl Relay {
    pub fn from_env() -> Result<Option<Self>, String> {
        let (Some(dir), Some(id)) = (std::env::var_os("GHOST_RELAY_DIR"), std::env::var("GHOST_RELAY_ID").ok()) else {
            return Ok(None);
        };
        if !valid_id(&id) {
            return Err("GHOST_RELAY_ID must be 1-32 letters, digits, '-' or '_'.".into());
        }
        Self::open(PathBuf::from(dir), id).map(Some)
    }

    pub fn open(dir: PathBuf, id: String) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot open the relay directory: {e}"))?;
        let now = now_ms();
        Ok(Self { dir, me: Peer { id, state: State::Busy, since: now, beat: 0 }, written: 0, read: 0, held: false, warned: false })
    }

    pub fn id(&self) -> &str {
        &self.me.id
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Records this host's state; writes on every change and as a heartbeat.
    pub fn publish(&mut self, state: State) -> Option<String> {
        self.publish_at(state, now_ms())
    }

    pub fn publish_at(&mut self, state: State, now: u64) -> Option<String> {
        if state != self.me.state {
            self.me.state = state;
            self.me.since = now;
        } else if now.saturating_sub(self.written) < BEAT_MS {
            return None;
        }
        self.me.beat = now;
        self.written = now;
        let path = self.file(&self.me.id);
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec(&self.me).unwrap_or_default();
        match std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, &path)) {
            Ok(()) => {
                self.warned = false;
                None
            }
            Err(e) if !self.warned => {
                self.warned = true;
                Some(format!("Relay: cannot write the host state: {e}"))
            }
            Err(_) => None,
        }
    }

    /// Whether another host has the lobby or the turn. Reads the directory at
    /// most once a second; an unreadable directory never blocks hosting.
    pub fn hold(&mut self) -> bool {
        self.hold_at(now_ms())
    }

    pub fn hold_at(&mut self, now: u64) -> bool {
        if now.saturating_sub(self.read) >= READ_MS {
            self.read = now;
            let mut me = self.me.clone();
            // Deciding whether to create: count this host as ready from its
            // first held attempt, so it keeps its place in the queue.
            if me.state == State::Busy {
                me.state = State::Ready;
                me.since = now;
            }
            self.held = hold(&me, &peers(&self.dir), now);
        }
        self.held
    }

}

impl Drop for Relay {
    /// A host that stops hosting leaves the queue at once.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.file(&self.me.id));
    }
}

fn peers(dir: &Path) -> Vec<Peer> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Peer>(&b).ok())
        .filter(|p| valid_id(&p.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, state: State, since: u64, beat: u64) -> Peer {
        Peer { id: id.into(), state, since, beat }
    }

    #[test]
    fn an_open_lobby_elsewhere_holds_everyone_else() {
        let me = peer("b", State::Ready, 10, 100);
        assert!(hold(&me, &[peer("a", State::Lobby, 50, 100)], 100));
        assert!(!hold(&me, &[peer("a", State::Busy, 50, 100)], 100));
    }

    #[test]
    fn the_longest_waiting_ready_host_goes_first_and_ids_break_ties() {
        let peers = [peer("a", State::Ready, 20, 100), peer("c", State::Ready, 10, 100)];
        assert!(hold(&peer("b", State::Ready, 15, 100), &peers, 100));
        assert!(!hold(&peer("c", State::Ready, 10, 100), &peers, 100));
        assert!(hold(&peer("b", State::Ready, 10, 100), &peers, 100));
        assert!(!hold(&peer("a", State::Ready, 10, 100), &[peer("b", State::Ready, 10, 100)], 100));
    }

    #[test]
    fn silent_or_future_peers_and_itself_are_ignored() {
        let me = peer("b", State::Ready, 10, 100);
        let now = 100 + STALE_MS;
        assert!(!hold(&me, &[peer("a", State::Lobby, 0, 100)], now));
        assert!(!hold(&me, &[peer("a", State::Lobby, 0, now + 2 * STALE_MS)], now));
        assert!(!hold(&me, &[peer("b", State::Lobby, 0, now)], now));
    }

    #[test]
    fn three_hosts_hand_the_lobby_on_through_files() {
        let dir = std::env::temp_dir().join(format!("ghost-relay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = Relay::open(dir.clone(), "a".into()).unwrap();
        let mut b = Relay::open(dir.clone(), "b".into()).unwrap();
        let mut c = Relay::open(dir.clone(), "c".into()).unwrap();
        let mut t = 1_000_000;
        // All three reach the menu; a was first, so b and c wait.
        a.publish_at(State::Ready, t);
        b.publish_at(State::Ready, t + 10);
        c.publish_at(State::Ready, t + 20);
        t += 2_000;
        assert!(!a.hold_at(t));
        assert!(b.hold_at(t) && c.hold_at(t));
        a.publish_at(State::Lobby, t);
        t += 2_000;
        assert!(b.hold_at(t) && c.hold_at(t));
        // a's game starts: b, who waited longer than c, takes over at once.
        a.publish_at(State::Busy, t);
        t += 2_000;
        assert!(!b.hold_at(t));
        assert!(c.hold_at(t));
        b.publish_at(State::Lobby, t);
        // a returns to the menu behind c.
        a.publish_at(State::Ready, t + 1);
        t += 2_000;
        assert!(a.hold_at(t) && c.hold_at(t));
        b.publish_at(State::Busy, t);
        t += 2_000;
        assert!(!c.hold_at(t));
        assert!(a.hold_at(t));
        // c crashes with its lobby open: after STALE_MS the queue moves on.
        c.publish_at(State::Lobby, t);
        t += STALE_MS + 2_000;
        a.publish_at(State::Ready, t);
        assert!(!a.hold_at(t));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_are_plain_names() {
        assert!(valid_id("bot-1") && valid_id("EU_2"));
        assert!(!valid_id("") && !valid_id("../x") && !valid_id(&"a".repeat(33)));
    }
}
