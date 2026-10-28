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
        // Warcraft drops a second message sent right after the first, so
        // English and Russian lines go out a second apart.
        self.next = now + 1000;
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
    pub lookup: crate::ladder::lookup::Lookup,
    stats: Vec<String>,
    admin: super::lobby_admin::LobbyAdmin,
    admin_actions: Vec<super::lobby_admin::Action>,
    /// Players turned away by the MMR gate, with the time of the last kick.
    gated: std::collections::BTreeMap<String, u64>,
}
impl Chat {
    pub fn new(path: Option<&Path>) -> Self {
        Self {
            commands: Default::default(),
            history: History::open(path),
            tracker: Default::default(),
            replies: Default::default(),
            color: None,
            lookup: Default::default(),
            stats: Vec::new(),
            admin: super::lobby_admin::LobbyAdmin::new(path.and_then(Path::parent)),
            admin_actions: Vec::new(),
            gated: Default::default(),
        }
    }
    /// A lobby admin holds the start (?stop or a reserved slot).
    pub fn admin_hold(&self) -> bool {
        self.admin.holds_start()
    }
    /// Kicks and slot edits requested by lobby admins since the last call.
    pub fn take_admin_actions(&mut self) -> Vec<super::lobby_admin::Action> {
        std::mem::take(&mut self.admin_actions)
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
        self.lookup = Default::default();
        self.stats.clear();
        self.admin_actions.clear();
    }
    pub fn warning(&mut self) -> Option<String> {
        self.history.warning()
    }
    pub fn started(&mut self, name: &str) {
        self.tracker.started(name, &mut self.history, unix_now());
    }

    /// Turns away players whose w3ladder MMR is outside the lobby's range.
    fn gate(
        &mut self,
        engine: &Engine,
        lobby: &crate::protocol::Lobby,
        now: u64,
        milliseconds: u64,
    ) {
        let (min, max) = (engine.config.welcome.min_mmr, engine.config.welcome.max_mmr);
        if min.is_none() && max.is_none() {
            self.gated.clear();
            return;
        }
        let present: Vec<(String, u8)> = lobby
            .players
            .iter()
            .filter(|p| {
                p.slot_status == 2 && p.slot_type == 0 && !p.is_self && p.name.contains('#')
            })
            .map(|p| (p.name.clone(), p.slot))
            .collect();
        self.gated
            .retain(|name, _| present.iter().any(|(n, _)| n == name));
        for (name, slot) in present {
            if self.admin.admins.contains(&name) {
                continue;
            }
            let Some(Some(mmr)) = self.lookup.mmr(&name, now) else {
                continue;
            };
            let low = min.is_some_and(|m| mmr < m);
            let high = max.is_some_and(|m| mmr > m);
            if !(low || high)
                || self
                    .gated
                    .get(&name)
                    .is_some_and(|at| now.saturating_sub(*at) < 5)
            {
                continue;
            }
            self.gated.insert(name.clone(), now);
            let range = match (min, max) {
                (Some(a), Some(b)) => format!("{a}-{b}"),
                (Some(a), None) => format!("{a}+"),
                (None, Some(b)) => format!("≤{b}"),
                (None, None) => unreachable!(),
            };
            self.replies.push(
                &crate::i18n::both(
                    format!("{name}: this lobby is for MMR {range} (you have {mmr})."),
                    format!("{name}: это лобби для MMR {range} (у вас {mmr})."),
                ),
                milliseconds,
            );
            self.admin_actions
                .push(super::lobby_admin::Action::Kick(slot));
        }
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
        let lobby = lobby.filter(|_| {
            matches!(
                engine.phase,
                Phase::Lobby | Phase::Countdown(_) | Phase::Starting(_)
            )
        });
        self.lookup.observe(lobby);
        if lobby.map(|l| l.lobby_name.as_str()) != Some(self.replies.lobby.as_str()) {
            self.stats.clear();
        }
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
        if let Some(lobby) = lobby {
            let (actions, lines) = self.admin.observe(lobby, now);
            self.admin_actions.extend(actions);
            for line in lines {
                self.replies.push(&line, milliseconds);
            }
            if let Some(event) = event
                && let Some((reply, actions, activity)) = self.admin.handle(event, lobby, now)
            {
                self.replies.push(&reply, milliseconds);
                self.admin_actions.extend(actions);
                outcome.activity.get_or_insert(activity);
            }
            self.gate(engine, lobby, now, milliseconds);
        }
        if let Some(tag) = outcome.stats.take()
            && !self.stats.contains(&tag)
            && self.stats.len() < 24
        {
            self.stats.push(tag);
        }
        self.stats.retain(|tag| self.lookup.contains(tag));
        if !engine
            .config
            .commands
            .enabled(crate::commands::ChatCommand::Stats)
        {
            self.stats.clear();
        }
        self.stats.retain(|tag| {
            if let Some(text) = self.lookup.query(tag, now) {
                self.replies.push(&format!("{tag}: {text}"), milliseconds);
                false
            } else {
                true
            }
        });
        if let Some(request) = outcome.color.take() {
            if engine.phase != Phase::Lobby || self.color.is_some() {
                self.replies.push("Color change unavailable during another edit or countdown. Try again in the lobby.\nСейчас цвет сменить нельзя: идёт другое изменение или отсчёт. Попробуйте позже.", milliseconds);
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
