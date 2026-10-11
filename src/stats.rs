//! Statistics are updated in memory. A separate writer owns disk persistence.
use crate::protocol::{Lobby, OBSERVER_TEAM};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
mod storage;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub name: String,
    pub games: u64,
    pub first_seen: u64,
    pub last_seen: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Book {
    pub version: u32,
    pub players: BTreeMap<String, Record>,
}
impl Default for Book {
    fn default() -> Self {
        Self {
            version: 1,
            players: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
pub struct History {
    book: Book,
    writer: Option<storage::Writer>,
    unavailable: bool,
    warning: Option<String>,
}

impl History {
    pub fn open(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        match storage::Writer::open(path) {
            Ok((writer, book)) => Self {
                book,
                writer: Some(writer),
                ..Self::default()
            },
            Err(_) => Self {
                unavailable: true,
                warning: Some(
                    "Statistics unavailable; existing history was preserved. Hosting continues."
                        .into(),
                ),
                ..Self::default()
            },
        }
    }

    pub fn warning(&mut self) -> Option<String> {
        self.warning
            .take()
            .or_else(|| self.writer.as_ref().and_then(storage::Writer::warning))
    }

    pub fn seen(&mut self, name: &str, now: u64, played: bool) {
        if self.unavailable || name.trim().is_empty() || name.len() > 160 {
            return;
        }
        let key = name.to_lowercase();
        let record = self
            .book
            .players
            .entry(key.clone())
            .or_insert_with(|| Record {
                name: name.into(),
                games: 0,
                first_seen: now,
                last_seen: now,
            });
        record.last_seen = record.last_seen.max(now);
        if played {
            record.games = record.games.saturating_add(1);
        }
        if let Some(writer) = &self.writer {
            writer.update(key, record.clone());
        }
    }

    pub fn describe(&self, name: &str) -> String {
        if self.unavailable {
            return "Statistics unavailable; existing history has been preserved.".into();
        }
        let record = self.book.players.get(&name.to_lowercase());
        match record {
            Some(r) => format!(
                "[{}] {} game(s) started in this bot's lobbies | first seen {} | last seen {} (UTC).",
                r.name,
                r.games,
                date(r.first_seen),
                date(r.last_seen)
            ),
            None => format!("No history for {name} on this bot yet."),
        }
    }

    pub fn resolve(&self, name: &str) -> Result<String, &'static str> {
        if self.unavailable {
            return Err("Statistics unavailable; existing history has been preserved.");
        }
        if let Some(record) = self.book.players.get(&name.to_lowercase()) {
            return Ok(record.name.clone());
        }
        let mut matches = self.book.players.values().filter(|r| {
            r.name
                .split('#')
                .next()
                .is_some_and(|short| short.eq_ignore_ascii_case(name))
        });
        let Some(record) = matches.next() else {
            return Err("No history for that player on this bot yet.");
        };
        if matches.next().is_some() {
            return Err("That name is ambiguous. Use the full BattleTag.");
        }
        Ok(record.name.clone())
    }
}

#[derive(Default)]
pub(crate) struct Tracker {
    lobby: String,
    present: BTreeSet<String>,
    playing: BTreeSet<String>,
    known: BTreeMap<i64, String>,
    updated: u64,
    counted: bool,
}
impl Tracker {
    pub fn observe(&mut self, lobby: &Lobby, history: &mut History, now: u64) {
        if self.lobby != lobby.lobby_name {
            *self = Self {
                lobby: lobby.lobby_name.clone(),
                ..Self::default()
            };
        }
        let people: Vec<_> = lobby
            .players
            .iter()
            .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
            .filter_map(|p| {
                let name = if p.name.trim().is_empty() {
                    // Profile resolution can briefly clear the name of an occupied slot.
                    self.known.get(&p.id.filter(|id| *id >= 0)?).cloned()?
                } else {
                    p.name.clone()
                };
                Some((p, name))
            })
            .collect();
        let present = people
            .iter()
            .map(|(_, name)| name.clone())
            .collect::<BTreeSet<_>>();
        for name in &present {
            if !self.present.contains(name) || now.saturating_sub(self.updated) >= 60 {
                history.seen(name, now, false);
            }
        }
        for name in self.present.difference(&present) {
            history.seen(name, now, false);
        }
        self.present = present;
        if now.saturating_sub(self.updated) >= 60 {
            self.updated = now;
        }
        self.known = people
            .iter()
            .filter_map(|(p, name)| p.id.filter(|id| *id >= 0).map(|id| (id, name.clone())))
            .collect();
        self.playing = people
            .iter()
            .filter(|(p, _)| !p.is_observer && p.team != OBSERVER_TEAM)
            .map(|(_, name)| name.clone())
            .collect();
    }
    pub fn started(&mut self, name: &str, history: &mut History, now: u64) {
        if self.counted || self.lobby != name {
            return;
        }
        self.counted = true;
        for player in &self.playing {
            history.seen(player, now, true);
        }
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn date(seconds: u64) -> String {
    // Proleptic Gregorian conversion from Unix days, without local timezone state.
    let z = (seconds / 86400).min(2_932_896) as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    format!("{:04}-{m:02}-{d:02}", y + i64::from(m <= 2))
}

#[cfg(test)]
mod tests;
