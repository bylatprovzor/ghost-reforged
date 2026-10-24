//! Record the native loading transition independently of result delivery.
mod queue;
#[cfg(test)]
mod tests;

use crate::{config::Config, ladder, protocol::Lobby};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Player {
    slot: u8,
    battletag: String,
    role: String,
    battle_net_region: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct StartedGame {
    version: u8,
    event: String,
    game_id: String,
    ladder_match_id: Option<String>,
    game_number: u32,
    bot_id: String,
    bot_name: String,
    game_name: String,
    map_name: String,
    map_version: String,
    started_at_ms: u64,
    mode: String,
    players: Vec<Player>,
}

struct Snapshot {
    name: String,
    mmr: bool,
    number: u32,
    match_id: Option<String>,
    players: Vec<Player>,
}

pub(super) struct Reporter {
    directory: PathBuf,
    bot_id: String,
    mode: String,
    snapshot: Option<Snapshot>,
    wake: mpsc::Sender<()>,
    notices: mpsc::Receiver<String>,
}

impl Reporter {
    pub(super) fn open(config: &Config) -> Result<Option<Self>, String> {
        let Some(key) =
            ladder::credential::read().map_err(|_| "Game reporting credential is unavailable.")?
        else {
            return Ok(None);
        };
        let mode = match std::env::var("W3LADDER_MODE").as_deref() {
            Ok("preview") => return Ok(None),
            Ok("test") => "test",
            Ok("ranked") | Err(_) => "live",
            _ => return Err("Game reporting mode is invalid.".into()),
        };
        let parent = config
            .stats_path
            .as_deref()
            .and_then(|p| p.parent())
            .map(|p| p.to_owned())
            .or_else(|| std::env::current_exe().ok()?.parent().map(|p| p.to_owned()))
            .ok_or("Game reporting directory is unavailable.")?;
        // Test and live queues never share credentials or retry each other's events.
        let directory = parent.join("game-reports").join(mode);
        let bot_id =
            queue::identity(&directory).map_err(|_| "Game reporting storage is unavailable.")?;
        let (wake, rx) = mpsc::channel();
        let (tx, notices) = mpsc::channel();
        let folder = directory.clone();
        thread::Builder::new()
            .name("ghost-game-reports".into())
            .spawn(move || queue::run(folder, key, rx, tx))
            .map_err(|_| "Game reporting worker could not start.")?;
        Ok(Some(Self {
            directory,
            bot_id,
            mode: mode.into(),
            snapshot: None,
            wake,
            notices,
        }))
    }

    pub(super) fn observe(&mut self, lobby: &Lobby, number: u32, match_id: Option<&str>) {
        if !ladder::matches_map(&lobby.map_data.map_name) {
            self.snapshot = None;
            return;
        }
        let mut players: Vec<_> = lobby
            .players
            .iter()
            .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_observer)
            .map(|p| Player {
                slot: p.slot,
                battletag: p.name.clone(),
                role: "player".into(),
                battle_net_region: match ladder::Region::from_player(p) {
                    ladder::Region::Europe => Some("eu".into()),
                    ladder::Region::Americas => Some("us".into()),
                    ladder::Region::Asia => Some("kr".into()),
                    ladder::Region::China => Some("cn".into()),
                    ladder::Region::Unknown => None,
                },
            })
            .collect();
        players.sort_by_key(|p| p.slot);
        // Never fabricate an unresolved BattleTag or a slot from the map's AIs.
        if players.is_empty() || players.iter().any(|p| !p.battletag.contains('#')) {
            self.snapshot = None;
            return;
        }
        self.snapshot = Some(Snapshot {
            name: lobby.lobby_name.clone(),
            mmr: ladder::uses_mmr_packet(&lobby.map_data.map_name),
            number,
            match_id: match_id.map(str::to_owned),
            players,
        });
    }

    pub(super) fn started(&mut self, name: &str) -> Result<(), String> {
        let Some(snapshot) = self.snapshot.as_ref().filter(|s| s.name == name) else {
            return Err("Started game has no complete reporting snapshot.".into());
        };
        let game = self.event(snapshot, millis()?)?;
        // Persist before the Warcraft process is restarted. HTTP stays off the hosting loop.
        queue::save(&self.directory, &game)
            .map_err(|_| "Started game could not be saved for reporting.")?;
        self.snapshot = None;
        let _ = self.wake.send(());
        Ok(())
    }

    fn event(&self, snapshot: &Snapshot, at: u64) -> Result<StartedGame, String> {
        Ok(StartedGame {
            version: 2,
            event: "game_started".into(),
            game_id: match &snapshot.match_id {
                Some(id) => id.clone(),
                None => queue::random_id().map_err(|_| "Cannot create game ID.")?,
            },
            ladder_match_id: snapshot.match_id.clone(),
            game_number: snapshot.number,
            bot_id: self.bot_id.clone(),
            bot_name: "GHost Reforged".into(),
            game_name: snapshot.name.clone(),
            map_name: if snapshot.mmr {
                ladder::MAP_NAME
            } else {
                ladder::LEGACY_MAP_NAME
            }
            .into(),
            map_version: if snapshot.mmr { "1.59a" } else { "1.59" }.into(),
            started_at_ms: at,
            mode: self.mode.clone(),
            players: snapshot.players.clone(),
        })
    }

    pub(super) fn notice(&self) -> Option<String> {
        self.notices.try_recv().ok()
    }
    pub(super) fn clear(&mut self) {
        self.snapshot = None;
    }
}

fn millis() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|_| "System clock cannot timestamp the game.".into())
}
