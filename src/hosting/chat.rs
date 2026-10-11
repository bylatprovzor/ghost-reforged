//! Native chat is handled on arrival; queued text and disk writes never sleep this loop.
use super::kick::{Refresh, RefreshGate};
use crate::{
    commands::{ColorRequest, LobbyCommands, Outcome},
    engine::{Engine, Phase},
    protocol::Command,
    stats::{History, Tracker, unix_now},
};
use serde_json::{Value, json};
use std::{collections::VecDeque, path::Path, time::Duration};

#[derive(Default)]
struct Replies {
    lobby: String,
    queued: VecDeque<(u64, String)>,
    next: u64,
}
impl Replies {
    fn lobby(&mut self, name: Option<&str>) {
        if name != Some(self.lobby.as_str()) {
            *self = Self {
                lobby: name.unwrap_or("").into(),
                ..Self::default()
            };
        }
    }
    fn push(&mut self, text: &str, now: u64) {
        for line in text.lines() {
            let clean = line.chars().filter(|c| !c.is_control()).collect::<String>();
            let mut rest = clean.trim();
            while !rest.is_empty() && self.queued.len() < 48 {
                let mut end = rest.len().min(240);
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                if end < rest.len()
                    && let Some(space) = rest[..end].rfind(' ')
                    && space > 100
                {
                    end = space;
                }
                self.queued.push_back((now, rest[..end].to_owned()));
                rest = rest[end..].trim_start();
            }
        }
    }
    fn pop(&mut self, now: u64) -> Option<Command> {
        while self
            .queued
            .front()
            .is_some_and(|(at, _)| now.saturating_sub(*at) > 10000)
        {
            self.queued.pop_front();
        }
        if now < self.next {
            return None;
        }
        let (_, text) = self.queued.pop_front()?;
        self.next = now + 150;
        Some(Command::new("SendGameChatMessage", json!({"content":text})))
    }
}

struct PendingColor {
    request: ColorRequest,
    gate: RefreshGate,
    sent: bool,
    since: u64,
}

pub(super) struct Chat {
    commands: LobbyCommands,
    history: History,
    tracker: Tracker,
    replies: Replies,
    color: Option<PendingColor>,
}
impl Chat {
    pub fn new(path: Option<&Path>) -> Self {
        Self {
            commands: Default::default(),
            history: History::open(path),
            tracker: Default::default(),
            replies: Default::default(),
            color: None,
        }
    }
    pub fn apply_settings(&mut self, settings: &crate::commands::CommandSettings) {
        self.commands.apply_settings(settings);
        if !settings.enabled(crate::commands::ChatCommand::Color) {
            self.color = None;
        }
    }
    pub fn reset(&mut self) {
        self.commands = Default::default();
        self.replies = Default::default();
        self.color = None;
    }
    pub fn warning(&mut self) -> Option<String> {
        self.history.warning()
    }
    pub fn started(&mut self, name: &str) {
        self.tracker.started(name, &mut self.history, unix_now());
    }

    pub fn process(
        &mut self,
        engine: &Engine,
        event: Option<&Value>,
        identity: bool,
        elapsed: Duration,
        actions: &mut Vec<Command>,
    ) -> Outcome {
        let now = elapsed.as_secs();
        let milliseconds = elapsed.as_millis() as u64;
        let roster = event.is_some_and(|e| e["messageType"] == "GameLobbySetup");
        let lobby = engine.owned_lobby(now).filter(|_| identity);
        self.replies.lobby(lobby.map(|l| l.lobby_name.as_str()));
        if roster && let Some(lobby) = lobby {
            self.tracker.observe(lobby, &mut self.history, unix_now());
        }
        let mut outcome = super::control::chat_command(
            engine,
            &mut self.commands,
            event,
            identity,
            elapsed,
            Some(&self.history),
        );
        if let Some(reply) = outcome.reply.take()
            && let Some(text) = reply.payload["content"].as_str()
        {
            self.replies.push(text, milliseconds);
        }
        if let Some(request) = outcome.color.take() {
            if engine.phase != Phase::Lobby || self.color.is_some() {
                self.replies.push("Color change unavailable during another edit or countdown. Try again in the lobby.", milliseconds);
            } else {
                self.color = Some(PendingColor {
                    request,
                    gate: Default::default(),
                    sent: false,
                    since: milliseconds,
                });
            }
        }
        if let Some(pending) = &mut self.color {
            let applied =
                pending.sent && roster && lobby.is_some_and(|l| pending.request.applied(l));
            let invalid = engine.phase != Phase::Lobby
                || !identity
                || milliseconds.saturating_sub(pending.since) >= 5000
                || lobby.is_none()
                || (!pending.sent && lobby.is_some_and(|l| pending.request.confirm(l).is_none()));
            if applied || invalid {
                self.replies
                    .push(&pending.request.response(applied), milliseconds);
                self.color = None;
            } else if !pending.sent {
                match pending.gate.advance(roster, event.is_none(), identity) {
                    Refresh::Request => actions.push(Command::empty("SendGameLobbySetup")),
                    Refresh::Confirm => {
                        if let Some(command) = lobby.and_then(|l| pending.request.confirm(l)) {
                            actions.push(command);
                            actions.push(Command::empty("SendGameLobbySetup"));
                            pending.sent = true;
                        }
                    }
                    Refresh::Wait => {}
                }
            }
        }
        if lobby.is_some() {
            actions.extend(self.replies.pop(milliseconds));
        }
        outcome
    }
}

#[cfg(test)]
mod tests;
