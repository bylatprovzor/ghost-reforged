//! Applies the locally measured inline host alias; this does not prove remote anonymity.
use crate::{protocol, transport::Client};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[cfg(windows)]
#[path = "alias_memory.rs"]
mod memory;

#[cfg(windows)]
#[path = "alias_candidates.rs"]
mod candidates;

#[cfg(test)]
#[path = "alias_tests.rs"]
mod tests;

pub enum Verification {
    Verified {
        snapshot: Value,
        pending: Vec<Value>,
    },
    Interrupted(Vec<Value>),
    Changed {
        snapshot: Value,
        pending: Vec<Value>,
    },
}

pub struct Alias {
    #[cfg(windows)]
    memory: memory::Memory,
    #[cfg(windows)]
    patch: Option<memory::Patch>,
    #[cfg(windows)]
    candidates: candidates::Candidates,
    name: String,
}

impl Alias {
    pub fn open(pid: u32, name: String) -> Result<Self, String> {
        if name.is_empty() || name.len() > 15 || !name.is_ascii() || name.contains('#') {
            return Err("Unsupported host name.".into());
        }
        #[cfg(windows)]
        return Ok(Self {
            memory: memory::Memory::open(pid)?,
            patch: None,
            candidates: candidates::Candidates::default(),
            name,
        });
        #[cfg(not(windows))]
        {
            let _ = pid;
            Err("Host aliases require Windows.".into())
        }
    }

    pub fn matches(&self, value: &Value) -> bool {
        value["playerHost"] == self.name && own_name(value) == Some(self.name.as_str())
    }

    /// Only call for a freshly confirmed lobby owned by this controller.
    pub fn ensure(
        &mut self,
        client: &mut Client,
        value: &Value,
        cancelled: &AtomicBool,
    ) -> Result<Verification, String> {
        self.ensure_observed(client, value, cancelled, &mut |_| {})
    }

    pub fn ensure_observed(
        &mut self,
        client: &mut Client,
        value: &Value,
        cancelled: &AtomicBool,
        observe: &mut impl FnMut(&Value),
    ) -> Result<Verification, String> {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(Verification::Interrupted(Vec::new()));
        }
        if self.matches(value) {
            return Ok(Verification::Verified {
                snapshot: value.clone(),
                pending: Vec::new(),
            });
        }
        let original = own_name(value).ok_or("Local host identity is unavailable.")?;
        if value["isHost"] != true || original == self.name {
            return Err("Host identity changed before alias verification.".into());
        }
        #[cfg(windows)]
        {
            self.restore();
            let deadline = Instant::now() + Duration::from_secs(90);
            let mut pending = Vec::new();
            let mut tried = std::collections::BTreeSet::new();
            for stage in 0..3 {
                if cancelled.load(Ordering::SeqCst) {
                    return Ok(Verification::Interrupted(pending));
                }
                let mut candidates = match (stage, self.candidates.verified) {
                    (0, Some(address)) => vec![address],
                    (1, Some(address)) => {
                        self.memory
                            .nearby_candidates(original.as_bytes(), address, cancelled)?
                    }
                    (2, _) => self.memory.candidates(original.as_bytes(), cancelled)?,
                    _ => Vec::new(),
                };
                if cancelled.load(Ordering::SeqCst) {
                    return Ok(Verification::Interrupted(pending));
                }
                self.candidates.rank(&mut candidates);
                if stage == 2 {
                    self.candidates.remember_scan(&candidates);
                }
                for address in candidates {
                    if !tried.insert(address) {
                        continue;
                    }
                    if cancelled.load(Ordering::SeqCst) {
                        return Ok(Verification::Interrupted(pending));
                    }
                    if Instant::now() >= deadline {
                        return Err("Host-name verification timed out.".into());
                    }
                    let Some(before) =
                        fresh_lobby_observed(client, value, cancelled, &mut pending, observe)?
                    else {
                        return Ok(Verification::Interrupted(pending));
                    };
                    if !same_roster(value, &before) {
                        return Ok(Verification::Changed {
                            snapshot: before,
                            pending,
                        });
                    }
                    if own_name(&before) != Some(original) {
                        return Err("Host identity changed during alias verification.".into());
                    }
                    let Some(patch) =
                        self.memory
                            .apply(address, original.as_bytes(), self.name.as_bytes())?
                    else {
                        continue;
                    };
                    self.patch = Some(patch);
                    let Some(after) =
                        fresh_lobby_observed(client, value, cancelled, &mut pending, observe)?
                    else {
                        return Ok(Verification::Interrupted(pending));
                    };
                    if self.matches(&after) {
                        self.candidates.verified = Some(address);
                        return Ok(Verification::Verified {
                            snapshot: after,
                            pending,
                        });
                    }
                    self.restore();
                    if !same_roster(value, &after) {
                        return Ok(Verification::Changed {
                            snapshot: after,
                            pending,
                        });
                    }
                }
            }
        }
        Err("The host name could not be applied to this Warcraft III lobby.".into())
    }

    pub fn restore(&mut self) {
        #[cfg(windows)]
        if let Some(patch) = self.patch.take() {
            self.memory.restore(patch);
        }
    }

    pub fn reset(&mut self) {
        self.restore();
        #[cfg(windows)]
        {
            self.candidates = candidates::Candidates::default();
        }
    }

    pub fn rename(&mut self, name: &str) -> Result<bool, String> {
        if name.trim().is_empty()
            || name.len() > 15
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b" _-+".contains(&b))
        {
            return Err("Unsupported host name.".into());
        }
        if self.name == name {
            return Ok(false);
        }
        self.restore();
        self.name = name.into();
        Ok(true)
    }

    /// Discard pre-rename serializations after restoring the previous alias.
    pub fn refresh_observed(
        &mut self,
        client: &mut Client,
        expected: &Value,
        cancelled: &AtomicBool,
        observe: &mut impl FnMut(&Value),
    ) -> Result<Verification, String> {
        let mut pending = Vec::new();
        match fresh_lobby_observed(client, expected, cancelled, &mut pending, observe)? {
            Some(snapshot) => Ok(Verification::Changed { snapshot, pending }),
            None => Ok(Verification::Interrupted(pending)),
        }
    }
}

fn same_roster(left: &Value, right: &Value) -> bool {
    let (Some(left), Some(right)) = (left["players"].as_array(), right["players"].as_array())
    else {
        return false;
    };
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            [
                "slot",
                "id",
                "slotStatus",
                "slotType",
                "team",
                "isSelf",
                "isObserver",
            ]
            .iter()
            .all(|key| left[*key] == right[*key])
        })
}

impl Drop for Alias {
    fn drop(&mut self) {
        self.restore();
    }
}

fn own_name(value: &Value) -> Option<&str> {
    let mut own = value["players"]
        .as_array()?
        .iter()
        .filter(|p| p["isSelf"] == true);
    let name = own.next()?["name"].as_str()?;
    own.next().is_none().then_some(name)
}

#[cfg(test)]
fn fresh_lobby(
    client: &mut Client,
    expected: &Value,
    cancelled: &AtomicBool,
    pending: &mut Vec<Value>,
) -> Result<Option<Value>, String> {
    fresh_lobby_observed(client, expected, cancelled, pending, &mut |_| {})
}

fn fresh_lobby_observed(
    client: &mut Client,
    expected: &Value,
    cancelled: &AtomicBool,
    pending: &mut Vec<Value>,
    observe: &mut impl FnMut(&Value),
) -> Result<Option<Value>, String> {
    // Drain earlier callbacks before requesting a new native serialization.
    let drain = Instant::now() + Duration::from_millis(50);
    while Instant::now() < drain {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(None);
        }
        match client.receive_available().map_err(|e| e.to_string())? {
            Some(event) => {
                if event["messageType"] == "GameLobbySetup" {
                    observe(&event["payload"]);
                }
                if defer_event(event, pending) {
                    return Ok(None);
                }
            }
            None => break,
        }
    }
    client
        .send("SendGameLobbySetup", json!({}))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let Some(event) = client.receive().map_err(|e| e.to_string())? else {
            continue;
        };
        if event["messageType"] != "GameLobbySetup" {
            if defer_event(event, pending) {
                return Ok(None);
            }
            continue;
        }
        let value = &event["payload"];
        if value["isHost"] != true
            || value["lobbyName"] != expected["lobbyName"]
            || !protocol::same_map_path(
                value["mapData"]["mapPath"].as_str().unwrap_or(""),
                expected["mapData"]["mapPath"].as_str().unwrap_or("!"),
            )
            || own_name(value).is_none()
        {
            return Err("Lobby ownership changed during alias verification.".into());
        }
        observe(value);
        return Ok(Some(value.clone()));
    }
    Err("The game did not confirm the host name.".into())
}

fn defer_event(event: Value, pending: &mut Vec<Value>) -> bool {
    let kind = event["messageType"].as_str().unwrap_or("");
    let interrupted = matches!(
        kind,
        "LoggedOut" | "MultiplayerGameLeave" | "GameClosed" | "ExitedGame"
    ) || (kind == "SetGlueScreen" && event["payload"]["screen"] != "GAME_LOBBY");
    if interrupted
        || matches!(
            kind,
            "LobbyCountdown" | "OnNetProviderChanged" | "OnNetProviderInitialized"
        )
    {
        pending.push(event);
    }
    interrupted
}
