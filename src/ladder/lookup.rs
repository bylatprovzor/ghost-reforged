//! Public player records for lobby chat; requests never block the hosting loop.
use super::{Error, Profile};
use crate::protocol::Lobby;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

pub const UNKNOWN: &str = "Wins: ???, Loss: ???, Rank: ???";
const MMR_UNKNOWN: &str = "MMR: ???, Rank: ???, Wins: ???, Loss: ???";

pub fn fetch_player(tag: &str) -> Result<Profile, Error> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent
        .get("https://api.w3ladder.net/v1/players")
        .query("battletag", tag)
        .call()
        .map_err(|_| Error::Unavailable)?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(16 * 1024)
        .read_to_string()
        .map_err(|_| Error::Unavailable)?;
    decode(status, &body, tag)
}

fn decode(status: u16, body: &str, tag: &str) -> Result<Profile, Error> {
    let mut value: serde_json::Value =
        serde_json::from_str(body).map_err(|_| Error::Unavailable)?;
    if status == 404 && value["error"] == "player_not_found" {
        return Ok(Profile {
            slot: 0,
            battletag: tag.into(),
            wins: 0,
            losses: 0,
            rank: None,
            mmr: Some(1500),
            mmr_games: Some(0),
        });
    }
    if status != 200 {
        return Err(Error::Unavailable);
    }
    value
        .as_object_mut()
        .ok_or(Error::InvalidRoster)?
        .insert("slot".into(), 0.into());
    let profile: Profile = serde_json::from_value(value).map_err(|_| Error::InvalidRoster)?;
    if !profile.battletag.eq_ignore_ascii_case(tag) || !profile.valid_rank() {
        return Err(Error::InvalidRoster);
    }
    Ok(profile)
}

struct Entry {
    job: Option<Receiver<Result<Profile, Error>>>,
    since: u64,
    text: String,
}

#[derive(Default)]
pub struct Lookup {
    mmr: bool,
    lobby: Option<(String, String)>,
    players: BTreeSet<String>,
    entries: BTreeMap<String, Entry>,
}

impl Lookup {
    pub fn observe(&mut self, lobby: Option<&Lobby>) {
        let mmr = lobby.is_some_and(|l| super::uses_mmr_packet(&l.map_data.map_name));
        let identity = lobby.map(|l| (l.lobby_name.clone(), l.map_data.map_path.clone()));
        if self.lobby != identity || self.mmr != mmr {
            *self = Self {
                lobby: identity,
                mmr,
                ..Self::default()
            };
        }
        self.players = lobby
            .into_iter()
            .flat_map(|l| &l.players)
            .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
            .map(|p| p.name.clone())
            .collect();
        self.entries.retain(|tag, _| self.players.contains(tag));
    }

    pub fn contains(&self, tag: &str) -> bool {
        self.players.contains(tag)
    }

    // None means a request is pending. Failures are explicit and cached briefly.
    pub fn query(&mut self, tag: &str, now: u64) -> Option<String> {
        let unknown = if self.mmr { MMR_UNKNOWN } else { UNKNOWN };
        if !self.contains(tag) || !tag.contains('#') {
            return Some(unknown.into());
        }
        if self
            .entries
            .get(tag)
            .is_some_and(|e| e.job.is_none() && now.saturating_sub(e.since) >= 60)
        {
            self.entries.remove(tag);
        }
        let entry = self.entries.entry(tag.into()).or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            let tag = tag.to_owned();
            let job = thread::Builder::new()
                .name("ladder-player".into())
                .spawn(move || {
                    let _ = tx.send(fetch_player(&tag));
                })
                .ok()
                .map(|_| rx);
            Entry {
                job,
                since: now,
                text: unknown.into(),
            }
        });
        if let Some(rx) = &entry.job {
            match rx.try_recv() {
                Ok(result) => {
                    entry.text = result.map_or_else(
                        |_| unknown.into(),
                        |p| {
                            if self.mmr {
                                p.mmr_summary()
                            } else {
                                p.summary()
                            }
                        },
                    );
                    entry.job = None;
                    entry.since = now;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    entry.job = None;
                    entry.since = now;
                }
                Err(mpsc::TryRecvError::Empty) if now.saturating_sub(entry.since) >= 10 => {
                    entry.job = None;
                    entry.since = now;
                }
                Err(mpsc::TryRecvError::Empty) => return None,
            }
        }
        Some(entry.text.clone())
    }
}

#[cfg(test)]
mod tests;
