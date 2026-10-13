use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default)]
    pub headless: bool,
    pub title: String,
    pub map: Option<PathBuf>,
    #[serde(default, rename = "bot_name", skip_serializing)]
    pub legacy_bot_name: serde::de::IgnoredAny,
    pub lobby_pattern: String,
    #[serde(default)]
    pub automatic_name: bool,
    pub next_number: u32,
    pub players: u8,
    pub countdown: u16,
    #[serde(default = "private_by_default", skip_serializing)]
    pub auto_start: bool,
    pub rehost: bool,
    #[serde(default = "private_by_default")]
    pub private_game: bool,
    #[serde(default)]
    pub speed: GameSpeed,
    #[serde(default = "private_by_default")]
    pub require_full: bool,
    #[serde(default)]
    pub commands: crate::commands::CommandSettings,
    #[serde(default)]
    pub welcome: crate::welcome::Welcome,
    #[serde(default = "rehost_delay")]
    pub rehost_delay_seconds: u64,
    #[serde(default)]
    pub empty_rehost_seconds: u64,
}

fn rehost_delay() -> u64 {
    15
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GameSpeed {
    Slow,
    Normal,
    #[default]
    Fast,
}

impl GameSpeed {
    pub const ALL: [Self; 3] = [Self::Slow, Self::Normal, Self::Fast];
    pub fn wire(self) -> u8 {
        match self {
            Self::Slow => 0,
            Self::Normal => 1,
            Self::Fast => 2,
        }
    }
}

impl std::fmt::Display for GameSpeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Slow => "Slow",
            Self::Normal => "Normal",
            Self::Fast => "Fast",
        })
    }
}

fn private_by_default() -> bool {
    true
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            headless: false,
            title: "Profile 1".into(),
            map: None,
            legacy_bot_name: serde::de::IgnoredAny,
            lobby_pattern: "Game #{n}".into(),
            automatic_name: true,
            next_number: 1,
            players: 4,
            countdown: 0,
            auto_start: true,
            rehost: true,
            private_game: true,
            speed: GameSpeed::Fast,
            require_full: true,
            commands: Default::default(),
            welcome: Default::default(),
            rehost_delay_seconds: rehost_delay(),
            empty_rehost_seconds: 0,
        }
    }
}

impl Profile {
    pub fn config(&self, directory: &std::path::Path) -> Result<crate::config::Config, String> {
        use crate::config::{Config, HostMode};
        self.validate()?;
        let map = crate::maps::validate(
            self.map
                .as_deref()
                .ok_or("Select a Warcraft III map first.")?,
        )?;
        Ok(Config {
            headless: self.headless,
            maps: vec![map],
            host_mode: HostMode::Observer,
            lobby_name: self.lobby_pattern.clone(),
            first_number: self.next_number,
            min_players: self.players,
            game_speed: self.speed.wire(),
            require_full: self.require_full,
            rehost_delay_seconds: self.rehost_delay_seconds,
            empty_rehost_seconds: self.empty_rehost_seconds,
            auto_start: true,
            commands: self.commands.clone(),
            stats_path: Some(directory.join("players.json")),
            welcome: self.welcome.clone(),
            resume_lobby: true,
            countdown_seconds: 0,
            leave_on_load: self.rehost,
            restart_on_load: self.rehost,
            private_game: self.private_game,
            max_games: if self.rehost { 0 } else { 1 },
            ..Config::default()
        })
    }

    pub fn live_settings(&self) -> crate::hosting::LiveSettings {
        crate::hosting::LiveSettings {
            headless: self.headless,
            minimum: self.players,
            full: self.require_full,
            rehost: self.rehost,
            rehost_delay: self.rehost_delay_seconds,
            empty_refresh: self.empty_rehost_seconds,
            commands: self.commands.clone(),
            welcome: self.welcome.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.welcome.validate()?;
        if self.title.trim().is_empty()
            || self.title.chars().count() > 60
            || self.title.chars().any(char::is_control)
        {
            return Err("Profile names must contain 1 to 60 characters.".into());
        }
        if !self.lobby_pattern.contains("{n}")
            || self.lobby_name(self.next_number).len() > 31
            || self.lobby_pattern.chars().any(char::is_control)
            || self.lobby_pattern.replace("{n}", "").contains(['{', '}'])
        {
            return Err(
                "Game names must contain {n} and fit in 31 UTF-8 bytes including the counter."
                    .into(),
            );
        }
        if !(1..=23).contains(&self.players) || self.countdown > 300 {
            return Err("Choose 1 to 23 players and a countdown from 0 to 300 seconds.".into());
        }
        if self.next_number == 0 {
            return Err("Next number must be at least 1.".into());
        }
        if !(5..=3600).contains(&self.rehost_delay_seconds) || self.empty_rehost_seconds > 86400 {
            return Err(
                "Rehost delay must be 5–3600 seconds; empty lobby refresh must be 0–86400 seconds."
                    .into(),
            );
        }
        if let Some(map) = &self.map {
            let ext = map.extension().and_then(|s| s.to_str()).unwrap_or("");
            if !ext.eq_ignore_ascii_case("w3x") && !ext.eq_ignore_ascii_case("w3m") {
                return Err("Select a .w3x or .w3m map.".into());
            }
        }
        Ok(())
    }

    pub fn lobby_name(&self, number: u32) -> String {
        self.lobby_pattern.replace("{n}", &number.to_string())
    }

    pub fn map_label(&self) -> String {
        self.map
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Select a .w3x or .w3m map".into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Profiles {
    pub version: u32,
    pub selected: usize,
    pub items: Vec<Profile>,
}

impl Default for Profiles {
    fn default() -> Self {
        Self {
            version: 2,
            selected: 0,
            items: vec![Profile::default()],
        }
    }
}

impl Profiles {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 2
            || self.items.is_empty()
            || self.items.len() > 30
            || self.selected >= self.items.len()
        {
            return Err("Unsupported profile format. The file has been preserved.".into());
        }
        self.items.iter().try_for_each(Profile::validate)
    }
}
