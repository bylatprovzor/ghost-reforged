use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommand {
    VoteStart,
    VoteCancel,
    VoteKick,
    Players,
    Help,
    Status,
    Yes,
    From,
    Map,
    Color,
    Stats,
}

impl ChatCommand {
    pub const ALL: [Self; 11] = [
        Self::VoteStart,
        Self::VoteCancel,
        Self::VoteKick,
        Self::Players,
        Self::Help,
        Self::Status,
        Self::Yes,
        Self::From,
        Self::Map,
        Self::Color,
        Self::Stats,
    ];
    pub const NEW: [Self; 6] = [
        Self::Status,
        Self::Yes,
        Self::From,
        Self::Map,
        Self::Color,
        Self::Stats,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::VoteStart => "?votestart",
            Self::VoteCancel => "?votecancel",
            Self::VoteKick => "?votekick",
            Self::Players => "?players",
            Self::Help => "?help",
            Self::Status => "?status",
            Self::Yes => "?yes",
            Self::From => "?from",
            Self::Map => "?map",
            Self::Color => "?color",
            Self::Stats => "?stats",
        }
    }

    pub fn read_only(self) -> bool {
        matches!(
            self,
            Self::Players | Self::Help | Self::Status | Self::From | Self::Map | Self::Stats
        )
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::VoteStart => {
                "Start with a majority vote. Observers do not vote; votes expire after 60 seconds."
            }
            Self::VoteCancel => "Withdraw your start and kick votes before they pass.",
            Self::VoteKick => {
                "Use ?votekick <name>. At least 3 players; a majority of the other players must agree within 60 seconds."
            }
            Self::Players => "Show the player and start vote counts.",
            Self::Help => "List enabled commands. Use ?help <command> for details.",
            Self::Status => "Show the start condition, waiting reason and current votes.",
            Self::Yes => "Join the active vote. Use ?yes start or ?yes kick if both are active.",
            Self::From => "Use ?from [name] to show Battle.net regions, not countries.",
            Self::Map => "Show the current map name and file, including its version when present.",
            Self::Color => "Use ?color <color> to change your own color when the map allows it.",
            Self::Stats => {
                "Use ?stats [name] for ladder wins, losses and rank on ranked SCR maps; local hosting history on other maps."
            }
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        let word = word.trim().trim_start_matches('?');
        Self::ALL
            .into_iter()
            .find(|c| c.name()[1..].eq_ignore_ascii_case(word))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CommandSettings(pub BTreeSet<ChatCommand>);

impl<'de> Deserialize<'de> for CommandSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = BTreeSet::<String>::deserialize(deserializer)?;
        let mut settings = BTreeSet::new();
        if names.contains("ready") || names.contains("unready") {
            settings.insert(ChatCommand::VoteKick);
        }
        for name in names {
            let command = match name.as_str() {
                "vote_start" => ChatCommand::VoteStart,
                "vote_cancel" => ChatCommand::VoteCancel,
                "vote_kick" => ChatCommand::VoteKick,
                "players" => ChatCommand::Players,
                "help" => ChatCommand::Help,
                "status" => ChatCommand::Status,
                "yes" => ChatCommand::Yes,
                "from" => ChatCommand::From,
                "map" => ChatCommand::Map,
                "color" => ChatCommand::Color,
                "stats" => ChatCommand::Stats,
                "ready" | "unready" => continue,
                _ => return Err(serde::de::Error::custom("Unknown chat command")),
            };
            settings.insert(command);
        }
        Ok(Self(settings))
    }
}

impl Default for CommandSettings {
    fn default() -> Self {
        Self(ChatCommand::ALL.into_iter().collect())
    }
}
impl CommandSettings {
    pub fn enabled(&self, command: ChatCommand) -> bool {
        self.0.contains(&command)
    }
    pub fn set(&mut self, command: ChatCommand, enabled: bool) {
        if enabled {
            self.0.insert(command);
        } else {
            self.0.remove(&command);
        }
    }
}
