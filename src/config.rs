use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Cannot read configuration: {0}")]
    Read(#[from] std::io::Error),
    #[error("Invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Invalid configuration: {0}")]
    Invalid(String),
    #[error(
        "Invisible hosting has not been validated on Battle.net 3.0. No lobby was created. Observer mode does not satisfy invisible hosting."
    )]
    InvisibleUnsupported,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub headless: bool,
    pub expected_version: String,
    pub maps: Vec<PathBuf>,
    pub lobby_name: String,
    pub first_number: u32,
    pub private_game: bool,
    pub lobby_password: String,
    pub host_mode: HostMode,
    pub game_speed: u8,
    pub min_players: u8,
    pub require_full: bool,
    pub count_host: bool,
    pub countdown_seconds: u64,
    pub auto_start: bool,
    pub commands: crate::commands::CommandSettings,
    pub stats_path: Option<PathBuf>,
    pub welcome: crate::welcome::Welcome,
    pub resume_lobby: bool,
    pub leave_on_load: bool,
    pub restart_on_load: bool,
    pub rehost_delay_seconds: u64,
    pub empty_rehost_seconds: u64,
    pub snapshot_interval_seconds: u64,
    pub stale_after_seconds: u64,
    pub command_timeout_seconds: u64,
    pub load_timeout_seconds: u64,
    pub max_games: u32,
    pub map_settings: MapSettings,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HostMode {
    Invisible,
    Observer,
    Player,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Observers {
    None,
    Defeated,
    Full,
    Referee,
}
impl Observers {
    pub fn wire(self) -> u32 {
        // Creation takes the dropdown index, not GameLobbySetup.typeObservers.
        match self {
            Self::None => 0,
            Self::Defeated => 1,
            Self::Full => 3,
            Self::Referee => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Default,
    Hidden,
    Explored,
    Visible,
}
impl Visibility {
    pub fn wire(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::Hidden => 1,
            Self::Explored => 2,
            Self::Visible => 3,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MapSettings {
    pub lock_teams: bool,
    pub teams_together: bool,
    pub shared_control: bool,
    pub random_races: bool,
    pub random_hero: bool,
    pub observers: Observers,
    pub visibility: Visibility,
}

impl Default for MapSettings {
    fn default() -> Self {
        Self {
            lock_teams: true,
            teams_together: true,
            shared_control: false,
            random_races: false,
            random_hero: false,
            observers: Observers::Full,
            visibility: Visibility::Default,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            headless: false,
            expected_version: "3.0.0.24268".into(),
            maps: Vec::new(),
            lobby_name: "GHost Rust #{n}".into(),
            first_number: 1,
            private_game: false,
            lobby_password: String::new(),
            host_mode: HostMode::Invisible,
            game_speed: 2,
            min_players: 2,
            require_full: false,
            count_host: false,
            countdown_seconds: 10,
            auto_start: true,
            commands: crate::commands::CommandSettings::default(),
            stats_path: None,
            welcome: crate::welcome::Welcome::default(),
            resume_lobby: false,
            leave_on_load: false,
            restart_on_load: false,
            rehost_delay_seconds: 15,
            empty_rehost_seconds: 0,
            snapshot_interval_seconds: 2,
            stale_after_seconds: 8,
            command_timeout_seconds: 30,
            load_timeout_seconds: 180,
            max_games: 0,
            map_settings: MapSettings::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let mut config: Self = toml::from_str(&fs::read_to_string(path)?)?;
        // Map paths are relative to the config, never the shell's current directory.
        let base = fs::canonicalize(path)?
            .parent()
            .expect("config file has a parent")
            .to_path_buf();
        for map in &mut config.maps {
            if map.is_relative() {
                *map = base.join(&*map);
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.welcome.validate().map_err(ConfigError::Invalid)?;
        let invalid = |s: &str| Err(ConfigError::Invalid(s.into()));
        if self.maps.is_empty() {
            return invalid("maps must contain at least one .w3x or .w3m file");
        }
        for map in &self.maps {
            let extension = map
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(extension.as_str(), "w3m" | "w3x") {
                return invalid("maps must be .w3x or .w3m files");
            }
            if !map.is_file() {
                return Err(ConfigError::Invalid(format!(
                    "map does not exist: {}",
                    map.display()
                )));
            }
        }
        if self.lobby_name.trim().is_empty()
            || self.lobby_name.contains(['\r', '\n', '\0'])
            || self.name(self.first_number).len() > 31
        {
            return invalid("lobby_name must be 1..31 UTF-8 bytes including the expanded {n}");
        }
        if !self.lobby_name.contains("{n}") {
            return invalid("lobby_name must contain {n} to identify each owned lobby");
        }
        if !(1..=24).contains(&self.min_players) || self.game_speed > 2 {
            return invalid("min_players must be 1..24 and game_speed 0..2");
        }
        if self.restart_on_load && (!self.leave_on_load || self.host_mode != HostMode::Observer) {
            return invalid("restart_on_load requires observer hosting with leave_on_load");
        }
        if self.snapshot_interval_seconds == 0
            || self.snapshot_interval_seconds > 30
            || self.stale_after_seconds <= self.snapshot_interval_seconds
        {
            return invalid(
                "snapshot interval must be 1..30 seconds; stale timeout must exceed it",
            );
        }
        if self.countdown_seconds > 300
            || !(5..=3600).contains(&self.rehost_delay_seconds)
            || !(5..=300).contains(&self.command_timeout_seconds)
            || !(10..=600).contains(&self.load_timeout_seconds)
            || self.stale_after_seconds > 300
            || self.empty_rehost_seconds > 86400
        {
            return invalid("timers outside supported bounds");
        }
        if self.host_mode == HostMode::Observer
            && !matches!(
                self.map_settings.observers,
                Observers::Full | Observers::Referee
            )
        {
            return invalid("observer host requires full or referee observers");
        }
        Ok(())
    }

    pub fn ensure_supported(&self) -> Result<(), ConfigError> {
        if self.host_mode == HostMode::Invisible {
            return Err(ConfigError::InvisibleUnsupported);
        }
        Ok(())
    }

    pub fn name(&self, number: u32) -> String {
        self.lobby_name.replace("{n}", &number.to_string())
    }
}
