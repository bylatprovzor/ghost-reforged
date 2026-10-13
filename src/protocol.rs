//! Wire fields verified against Warcraft III 3.0.0.24268 GlueManager.
use crate::config::Config;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const OBSERVER_TEAM: u8 = 24;

#[derive(Default)]
pub struct CreationEvidence {
    started: bool,
    entered_lobby: bool,
    result_success: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CreationError {
    #[error("Warcraft III rejected lobby creation")]
    Rejected,
    #[error("Lobby creation result schema changed")]
    InvalidResult,
    #[error("Warcraft III left while lobby creation was pending")]
    Left,
}

impl CreationEvidence {
    // Use a fresh instance after each locally issued CreateLobby. A snapshot
    // alone can be a preparation placeholder and is never proof of creation.
    pub fn observe(&mut self, event: &Value) -> Result<(), CreationError> {
        match event["messageType"].as_str() {
            Some("MultiplayerGameCreateStarted") => self.started = true,
            Some("SetGlueScreen") if self.started => {
                self.entered_lobby = event["payload"]["screen"] == "GAME_LOBBY";
            }
            Some("MultiplayerGameCreateResult") => {
                let success = &event["payload"]["details"]["success"];
                if success == true || success == 1 {
                    self.result_success = true;
                } else if success == false || success == 0 {
                    return Err(CreationError::Rejected);
                } else {
                    return Err(CreationError::InvalidResult);
                }
            }
            Some("MultiplayerGameLeave") => return Err(CreationError::Left),
            _ => {}
        }
        Ok(())
    }

    pub fn confirmed(&self) -> bool {
        self.result_success || (self.started && self.entered_lobby)
    }
}

pub fn same_map_path(left: &str, right: &str) -> bool {
    left.replace('/', "\\").to_lowercase() == right.replace('/', "\\").to_lowercase()
}

// The installed multiplayer UI accepts either the selected provider or the
// asynchronous online initialization result. Missing fields are not success.
pub fn bnet_confirmation(event: &Value) -> Option<bool> {
    match event["messageType"].as_str()? {
        "OnNetProviderChanged" => Some(event["payload"]["providerId"] == "BNET"),
        "OnNetProviderInitialized" => Some(event["payload"]["message"]["isOnline"] == true),
        _ => None,
    }
}

// An already initialized provider does not repeat its initialization callback.
// Battle.net listings carry a server region; LAN/empty lists are not evidence.
pub fn has_online_listing(payload: &Value) -> bool {
    payload["games"].as_array().is_some_and(|games| {
        games.iter().any(|game| {
            game["region"].as_str().is_some_and(|region| {
                ["eu", "usw", "use", "kr"]
                    .iter()
                    .any(|known| region.eq_ignore_ascii_case(known))
            }) && game["id"].as_u64().is_some_and(|id| id > 0)
                && game["currentPlayers"].as_u64().is_some()
                && game["maxPlayers"].as_u64().is_some_and(|count| count > 0)
        })
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lobby {
    #[serde(default, deserialize_with = "optional_metadata")]
    pub is_custom_players: bool,
    #[serde(default)]
    pub available_team_colors: Value,
    pub is_host: bool,
    pub lobby_name: String,
    pub players: Vec<Player>,
    pub map_data: MapData,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapData {
    pub map_path: String,
    #[serde(default, deserialize_with = "optional_metadata")]
    pub map_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    #[serde(default, deserialize_with = "optional_metadata")]
    pub handicap: Option<u8>,
    #[serde(default, deserialize_with = "optional_metadata")]
    pub race: Option<u8>,
    #[serde(default, deserialize_with = "optional_metadata")]
    pub player_region: String,
    #[serde(default, deserialize_with = "optional_metadata")]
    pub gateway_id: Option<i32>,
    #[serde(default, deserialize_with = "optional_metadata")]
    pub color: Option<u8>,
    #[serde(default, deserialize_with = "optional_metadata")]
    pub color_change_enabled: bool,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: String,
    pub slot: u8,
    pub slot_status: u8,
    pub slot_type: u8,
    pub team: u8,
    pub is_self: bool,
    pub is_observer: bool,
    pub team_change_enabled: bool,
    #[serde(default)]
    pub slot_type_change_enabled: bool,
}

// Optional chat metadata must never discard the roster used to cancel a start.
fn optional_metadata<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

impl Lobby {
    pub fn start_signature(&self) -> String {
        self.players
            .iter()
            .map(|p| {
                format!(
                    "{}:{:?}:{}:{}:{}:{};",
                    p.slot, p.id, p.name, p.slot_status, p.slot_type, p.team
                )
            })
            .collect()
    }
    pub fn local(&self) -> Option<&Player> {
        self.players.iter().find(|p| p.is_self)
    }
    pub fn humans(&self, count_host: bool) -> usize {
        self.players
            .iter()
            .filter(|p| {
                p.slot_status == 2
                    && p.slot_type == 0
                    && !p.is_observer
                    && p.team != OBSERVER_TEAM
                    && (count_host || !p.is_self)
            })
            .count()
    }
    pub fn full(&self) -> bool {
        !self
            .players
            .iter()
            .any(|p| p.slot_status == 0 && !p.is_observer && p.team != OBSERVER_TEAM)
    }
    pub fn occupied_players(&self) -> usize {
        self.players
            .iter()
            .filter(|p| p.slot_status == 2 && !p.is_observer && p.team != OBSERVER_TEAM)
            .count()
    }
    pub fn empty(&self) -> bool {
        !self
            .players
            .iter()
            .any(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Command {
    pub message: &'static str,
    pub payload: Value,
}
impl Command {
    pub fn new(message: &'static str, payload: Value) -> Self {
        Self { message, payload }
    }
    pub fn empty(message: &'static str) -> Self {
        Self::new(message, json!({}))
    }
}

pub fn create_lobby(config: &Config, number: u32, index: usize) -> Command {
    let settings = &config.map_settings;
    let mut payload = json!({
        "filename": client_path(&config.maps[index]),
        "gameName": config.name(number),
        "gameSpeed": config.game_speed,
        "privateGame": config.private_game,
        "mapSettings": {
            "flagLockTeams": settings.lock_teams,
            "flagPlaceTeamsTogether": settings.teams_together,
            "flagFullSharedUnitControl": settings.shared_control,
            "flagRandomRaces": settings.random_races,
            "flagRandomHero": settings.random_hero,
            "settingObservers": settings.observers.wire(),
            "settingVisibility": settings.visibility.wire(),
        }
    });
    if !config.lobby_password.is_empty() {
        payload["password"] = json!(config.lobby_password);
    }
    Command::new("CreateLobby", payload)
}

pub fn client_path(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    // Match GetMapList.filepath. In 3.0.0.24268 a backslash path can
    // create an empty lobby placeholder even when the file exists.
    if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        return format!("//{}", unc.replace('\\', "/"));
    }
    path.strip_prefix("\\\\?\\")
        .unwrap_or(&path)
        .replace('\\', "/")
}
