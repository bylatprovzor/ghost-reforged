//! Lobby commands use authenticated native senders, never names parsed from chat text.
use crate::protocol::{Command, Lobby, OBSERVER_TEAM};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod settings;
pub use settings::{ChatCommand, CommandSettings};
mod color;
mod info;
pub use color::ColorRequest;

#[derive(Default)]
pub struct Context<'a> {
    pub status: &'a str,
    pub history: Option<&'a crate::stats::History>,
}

#[derive(Default)]
pub struct Outcome {
    pub start: bool,
    pub kick: Option<KickTarget>,
    pub color: Option<ColorRequest>,
    pub stats: Option<String>,
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
    electorate: BTreeSet<PlayerKey>,
}

impl KickTarget {
    pub fn confirm(&self, lobby: &Lobby) -> Option<Command> {
        (lobby.is_host
            && lobby.lobby_name == self.lobby
            && electorate(lobby) == self.electorate
            && self.electorate.len() == lobby.humans(false)
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
    current: Option<Lobby>,
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
        self.current = Some(lobby.clone());
        let roster = electorate(lobby);
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
        self.handle_at(
            event,
            settings,
            now.saturating_mul(1000),
            Context::default(),
        )
    }

    pub fn handle_at(
        &mut self,
        event: &Value,
        settings: &CommandSettings,
        milliseconds: u64,
        context: Context<'_>,
    ) -> Outcome {
        let now = milliseconds / 1000;
        if event["messageType"] != "ChatMessage" {
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
        let argument = argument.trim();
        if !settings.enabled(command)
            || (!matches!(
                command,
                ChatCommand::VoteKick
                    | ChatCommand::Yes
                    | ChatCommand::From
                    | ChatCommand::Stats
                    | ChatCommand::Color
                    | ChatCommand::Help
            ) && !argument.is_empty())
        {
            return Outcome::default();
        }
        let Some(sender) = message["sender"].as_str() else {
            return Outcome::default();
        };
        let Some(lobby) = &self.current else {
            return Outcome::default();
        };
        if message["channelName"]
            .as_str()
            .is_some_and(|name| name != lobby.lobby_name)
        {
            return Outcome::default();
        }
        let participants: BTreeSet<_> = lobby
            .players
            .iter()
            .filter(|p| {
                p.slot_status == 2 && p.slot_type == 0 && !p.is_self && !p.name.trim().is_empty()
            })
            .map(|p| PlayerKey {
                id: p.id.filter(|id| *id >= 0),
                name: p.name.clone(),
            })
            .collect();
        let mut candidates = participants.iter().filter(|key| key.name == sender);
        let Some(key) = candidates.next().cloned() else {
            return Outcome::default();
        };
        if candidates.next().is_some()
            || self
                .last_command
                .get(&key)
                .is_some_and(|last| milliseconds.saturating_sub(*last) < 200)
        {
            return Outcome::default();
        }
        if !command.read_only() && (!self.complete_roster || !self.roster.contains(&key)) {
            return Outcome::default();
        }
        self.last_command.insert(key.clone(), milliseconds);
        let required = self.roster.len() / 2 + 1;
        let mut start = false;
        let mut kick = None;
        let mut color = None;
        let mut stats = None;
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
            ChatCommand::Help => info::help(settings, argument),
            ChatCommand::Status => format!(
                "{} {}{}",
                context.status,
                self.counts(required),
                self.kick_status()
            ),
            ChatCommand::From => info::regions(self.current.as_ref().unwrap(), argument),
            ChatCommand::Map => info::map(self.current.as_ref().unwrap()),
            ChatCommand::Stats if crate::ladder::matches_map(&lobby.map_data.map_name) => {
                match info::ladder_target(lobby, &key.name, argument) {
                    Ok(tag) => {
                        stats = Some(tag);
                        String::new()
                    }
                    Err(message) => message.into(),
                }
            }
            ChatCommand::Stats => info::stats(context.history, &key.name, argument),
            ChatCommand::Color => {
                let (reply, request) =
                    color::request(self.current.as_ref().unwrap(), &key.name, argument);
                color = request;
                reply
            }
            ChatCommand::Yes => {
                let start_active =
                    !self.votes.is_empty() && settings.enabled(ChatCommand::VoteStart);
                let kick_active = self.kick.is_some() && settings.enabled(ChatCommand::VoteKick);
                match argument.to_ascii_lowercase().as_str() {
                    "" if start_active && kick_active => {
                        "Two votes are active. Use ?yes start or ?yes kick.".into()
                    }
                    "" | "start" if start_active && argument != "kick" => {
                        self.votes.insert(key.clone());
                        start = self.votes.len() >= required;
                        if start {
                            "Vote passed. Starting the countdown.".into()
                        } else {
                            format!(
                                "Start vote: {}/{} needed. Vote with ?yes.",
                                self.votes.len(),
                                required
                            )
                        }
                    }
                    "" | "kick" if kick_active => {
                        let target = self.kick.as_ref().unwrap().target.name.clone();
                        let (reply, request) = self.vote_kick(&key, &target, now);
                        kick = request;
                        reply
                    }
                    "" | "start" | "kick" => "No matching vote is active.".into(),
                    _ => "Use ?yes, ?yes start or ?yes kick.".into(),
                }
            }
        };
        Outcome {
            start,
            kick,
            color,
            stats,
            reply: (!reply.is_empty())
                .then(|| Command::new("SendGameChatMessage", json!({"content":reply}))),
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
                    electorate: self.roster.clone(),
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

    fn kick_status(&self) -> String {
        self.kick
            .as_ref()
            .map(|v| {
                format!(
                    " Kick {}: {}/{} votes.",
                    v.target.name,
                    v.voters.len(),
                    (self.roster.len() - 1) / 2 + 1
                )
            })
            .unwrap_or_default()
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

fn electorate(lobby: &Lobby) -> BTreeSet<PlayerKey> {
    lobby
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
        .collect()
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
