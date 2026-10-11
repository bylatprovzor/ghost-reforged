//! Lobby commands use authenticated native senders, never names parsed from chat text.
use crate::protocol::{Command, Lobby, OBSERVER_TEAM};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommand {
    VoteStart,
    VoteCancel,
    VoteKick,
    Players,
    Help,
}

impl ChatCommand {
    pub const ALL: [Self; 5] = [
        Self::VoteStart,
        Self::VoteCancel,
        Self::VoteKick,
        Self::Players,
        Self::Help,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::VoteStart => "?votestart",
            Self::VoteCancel => "?votecancel",
            Self::VoteKick => "?votekick",
            Self::Players => "?players",
            Self::Help => "?help",
        }
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
            Self::Help => "List enabled commands.",
        }
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

#[derive(Default)]
pub struct Outcome {
    pub start: bool,
    pub kick: Option<KickTarget>,
    pub reply: Option<Command>,
    pub activity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PlayerKey {
    id: Option<i64>,
    name: String,
}

struct KickVote {
    target: PlayerKey,
    voters: BTreeSet<PlayerKey>,
    until: u64,
}

pub struct KickTarget {
    lobby: String,
    player: PlayerKey,
    slot: u8,
}

impl KickTarget {
    pub fn confirm(&self, lobby: &Lobby) -> Option<Command> {
        (lobby.is_host
            && lobby.lobby_name == self.lobby
            && lobby.players.iter().any(|p| {
                p.slot == self.slot
                    && p.id.filter(|id| *id >= 0) == self.player.id
                    && p.name == self.player.name
                    && p.slot_status == 2
                    && p.slot_type == 0
                    && !p.is_self
                    && !p.is_observer
                    && p.team != OBSERVER_TEAM
            }))
        .then(|| Command::new("KickPlayerFromGameLobby", json!({"slot":self.slot})))
    }
}

#[derive(Default)]
pub struct LobbyCommands {
    lobby: String,
    roster: BTreeSet<PlayerKey>,
    votes: BTreeSet<PlayerKey>,
    kick: Option<KickVote>,
    slots: BTreeMap<PlayerKey, u8>,
    complete_roster: bool,
    last_command: BTreeMap<PlayerKey, u64>,
    vote_deadline: Option<u64>,
    last_reply: Option<u64>,
}

impl LobbyCommands {
    pub fn apply_settings(&mut self, settings: &CommandSettings) {
        if !settings.enabled(ChatCommand::VoteStart) {
            self.votes.clear();
            self.vote_deadline = None;
        }
        if !settings.enabled(ChatCommand::VoteKick) {
            self.kick = None;
        }
    }

    pub fn observe(&mut self, lobby: Option<&Lobby>, now: u64) {
        let Some(lobby) = lobby else {
            *self = Self::default();
            return;
        };
        if self.lobby != lobby.lobby_name {
            *self = Self {
                lobby: lobby.lobby_name.clone(),
                ..Self::default()
            };
        }
        let roster: BTreeSet<_> = lobby
            .players
            .iter()
            .filter(|p| {
                p.slot_status == 2
                    && p.slot_type == 0
                    && !p.is_self
                    && !p.is_observer
                    && p.team != OBSERVER_TEAM
                    && !p.name.trim().is_empty()
            })
            .map(|p| PlayerKey {
                id: p.id.filter(|id| *id >= 0),
                name: p.name.clone(),
            })
            .collect();
        self.complete_roster = roster.len() == lobby.humans(false);
        self.slots = lobby
            .players
            .iter()
            .filter_map(|p| {
                let key = PlayerKey {
                    id: p.id.filter(|id| *id >= 0),
                    name: p.name.clone(),
                };
                roster.contains(&key).then_some((key, p.slot))
            })
            .collect();
        if self.kick.as_ref().is_some_and(|vote| now >= vote.until) {
            self.kick = None;
        }
        // A changed electorate needs a new vote; departure alone cannot pass one.
        if roster != self.roster {
            self.votes.clear();
            self.vote_deadline = None;
            self.kick = None;
            self.last_command.retain(|key, _| roster.contains(key));
            self.roster = roster;
        }
        if self.vote_deadline.is_some_and(|deadline| now >= deadline) {
            self.votes.clear();
            self.vote_deadline = None;
        }
    }

    pub fn handle(&mut self, event: &Value, settings: &CommandSettings, now: u64) -> Outcome {
        if !self.complete_roster || event["messageType"] != "ChatMessage" {
            return Outcome::default();
        }
        let message = &event["payload"]["message"];
        if message["source"] != "gameChat" {
            return Outcome::default();
        }
        let Some(content) = message["content"].as_str() else {
            return Outcome::default();
        };
        let (keyword, argument) = content
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((content.trim(), ""));
        let Some(command) = ChatCommand::ALL
            .into_iter()
            .find(|c| keyword.eq_ignore_ascii_case(c.name()))
        else {
            return Outcome::default();
        };
        if !settings.enabled(command) || (command != ChatCommand::VoteKick && !argument.is_empty())
        {
            return Outcome::default();
        }
        let Some(sender) = message["sender"].as_str() else {
            return Outcome::default();
        };
        let mut candidates = self.roster.iter().filter(|key| key.name == sender);
        let Some(key) = candidates.next().cloned() else {
            return Outcome::default();
        };
        if candidates.next().is_some()
            || self.last_command.get(&key).is_some_and(|last| now <= *last)
        {
            return Outcome::default();
        }
        self.last_command.insert(key.clone(), now);
        let required = self.roster.len() / 2 + 1;
        let mut start = false;
        let mut kick = None;
        let reply = match command {
            ChatCommand::VoteStart => {
                self.vote_deadline.get_or_insert(now + 60);
                self.votes.insert(key.clone());
                start = self.votes.len() >= required;
                if start {
                    "Vote passed. Starting the countdown.".into()
                } else {
                    format!(
                        "Start vote: {}/{} needed. Vote with ?votestart.",
                        self.votes.len(),
                        required
                    )
                }
            }
            ChatCommand::VoteCancel => {
                self.votes.remove(&key);
                if let Some(vote) = &mut self.kick {
                    vote.voters.remove(&key);
                }
                format!(
                    "Your votes were withdrawn. Start vote: {}/{} needed.",
                    self.votes.len(),
                    required
                )
            }
            ChatCommand::VoteKick => {
                let (message, target) = self.vote_kick(&key, argument.trim(), now);
                kick = target;
                message
            }
            ChatCommand::Players => self.counts(required),
            ChatCommand::Help => ChatCommand::ALL
                .into_iter()
                .filter(|c| settings.enabled(*c))
                .map(ChatCommand::name)
                .collect::<Vec<_>>()
                .join("  "),
        };
        let send = start || kick.is_some() || self.last_reply.is_none_or(|last| now > last);
        if send {
            self.last_reply = Some(now);
        }
        Outcome {
            start,
            kick,
            reply: send.then(|| Command::new("SendGameChatMessage", json!({"content":reply}))),
            activity: Some(format!("{sender} · {}", command.name())),
        }
    }

    fn vote_kick(
        &mut self,
        sender: &PlayerKey,
        name: &str,
        now: u64,
    ) -> (String, Option<KickTarget>) {
        if self.roster.len() < 3 {
            return ("Kick votes need at least 3 players.".into(), None);
        }
        let mut matches = self.roster.iter().filter(|p| {
            p.name.eq_ignore_ascii_case(name)
                || p.name
                    .split('#')
                    .next()
                    .is_some_and(|short| short.eq_ignore_ascii_case(name))
        });
        let Some(target) = matches.next().cloned() else {
            return ("Use ?votekick <player name>.".into(), None);
        };
        if matches.next().is_some() {
            return (
                "That name is ambiguous. Use the full player name.".into(),
                None,
            );
        }
        if &target == sender {
            return ("You cannot vote to kick yourself.".into(), None);
        }
        if self.kick.as_ref().is_some_and(|vote| vote.target != target) {
            return (
                "A kick vote is already open for another player.".into(),
                None,
            );
        }
        let vote = self.kick.get_or_insert_with(|| KickVote {
            target: target.clone(),
            voters: BTreeSet::new(),
            until: now + 60,
        });
        vote.voters.insert(sender.clone());
        let needed = (self.roster.len() - 1) / 2 + 1;
        if vote.voters.len() >= needed {
            let slot = self.slots[&target];
            self.kick = None;
            (
                format!("Kick vote passed for {}.", target.name),
                Some(KickTarget {
                    lobby: self.lobby.clone(),
                    player: target,
                    slot,
                }),
            )
        } else {
            (
                format!(
                    "Kick {}: {}/{} votes. Use ?votekick {}.",
                    target.name,
                    vote.voters.len(),
                    needed,
                    target.name
                ),
                None,
            )
        }
    }

    fn counts(&self, required: usize) -> String {
        format!(
            "Players: {}. Start votes: {}/{}.",
            self.roster.len(),
            self.votes.len(),
            required
        )
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
