//! Lobby admins may kick a player, hold the start and reserve a slot.
//!
//! An admin is a BattleTag listed in `lobby-admins.txt` (one per line, next to
//! the profile store) or granted "lobby admin" on w3ladder.net. Senders are the
//! authenticated native chat senders, never names typed into the message.
use crate::{
    i18n::both,
    protocol::{Lobby, OBSERVER_TEAM, Player},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

const READ_SECONDS: u64 = 30;
const FETCH_SECONDS: u64 = 60;
/// Wait for an applied slot or kick before asking for the next one.
const SETTLE_SECONDS: u64 = 3;
pub const FILE: &str = "lobby-admins.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Kick(u8),
    Close(u8),
    Open(u8),
}

#[derive(Default)]
pub(super) struct Admins {
    file: Option<PathBuf>,
    local: BTreeSet<String>,
    remote: BTreeSet<String>,
    read_at: Option<u64>,
    fetch_at: Option<u64>,
    job: Option<Receiver<Option<BTreeSet<String>>>>,
    online: bool,
}

fn normal(tag: &str) -> String {
    tag.trim().to_lowercase()
}

/// One BattleTag per line; empty lines and lines starting with ';' are skipped.
pub(super) fn parse_list(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|line| line.trim().trim_start_matches('\u{feff}'))
        .filter(|line| !line.is_empty() && !line.starts_with(';') && line.contains('#'))
        .map(normal)
        .collect()
}

fn fetch() -> Option<BTreeSet<String>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent
        .get("https://api.w3ladder.net/v1/lobby-admins")
        .call()
        .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .ok()?;
    let value: Value = serde_json::from_str(&body).ok()?;
    Some(
        value["battletags"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| t.contains('#') && t.len() <= 40)
            .map(normal)
            .collect(),
    )
}

impl Admins {
    pub fn new(directory: Option<&Path>) -> Self {
        Self {
            file: directory.map(|d| d.join(FILE)),
            online: directory.is_some(),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub fn with(tags: &[&str]) -> Self {
        Self {
            local: tags.iter().map(|t| normal(t)).collect(),
            ..Self::default()
        }
    }

    pub fn refresh(&mut self, now: u64) {
        if let Some(file) = &self.file
            && self
                .read_at
                .is_none_or(|at| now.saturating_sub(at) >= READ_SECONDS)
        {
            self.read_at = Some(now);
            self.local = std::fs::read_to_string(file)
                .map(|text| parse_list(&text))
                .unwrap_or_default();
        }
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok(list) => {
                    if let Some(list) = list {
                        self.remote = list;
                    }
                    self.job = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.job = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.online
            && self.job.is_none()
            && self
                .fetch_at
                .is_none_or(|at| now.saturating_sub(at) >= FETCH_SECONDS)
        {
            self.fetch_at = Some(now);
            let (tx, rx) = mpsc::channel();
            if thread::Builder::new()
                .name("lobby-admins".into())
                .spawn(move || {
                    let _ = tx.send(fetch());
                })
                .is_ok()
            {
                self.job = Some(rx);
            }
        }
    }

    pub fn contains(&self, tag: &str) -> bool {
        let tag = normal(tag);
        self.local.contains(&tag) || self.remote.contains(&tag)
    }
}

struct Reserve {
    player: String,
    by: String,
    slot: u8,
}

#[derive(Default)]
pub(super) struct LobbyAdmin {
    pub admins: Admins,
    lobby: String,
    hold: Option<String>,
    reserve: Option<Reserve>,
    settle_until: u64,
}

/// Players who can send chat: humans in the lobby other than the bot.
fn humans(lobby: &Lobby) -> impl Iterator<Item = &Player> {
    lobby.players.iter().filter(|p| {
        p.slot_status == 2 && p.slot_type == 0 && !p.is_self && !p.name.trim().is_empty()
    })
}

fn same(name: &str, wanted: &str) -> bool {
    name.eq_ignore_ascii_case(wanted)
        || (!wanted.contains('#')
            && name
                .split('#')
                .next()
                .is_some_and(|n| n.eq_ignore_ascii_case(wanted)))
}

fn playing(p: &Player) -> bool {
    !p.is_observer && p.team != OBSERVER_TEAM && !p.is_self
}

impl LobbyAdmin {
    pub fn new(directory: Option<&Path>) -> Self {
        Self {
            admins: Admins::new(directory),
            ..Self::default()
        }
    }

    /// Whether auto start and start votes must wait.
    pub fn holds_start(&self) -> bool {
        self.hold.is_some() || self.reserve.is_some()
    }

    fn settled(&mut self, now: u64) -> bool {
        if now < self.settle_until {
            return false;
        }
        self.settle_until = now + SETTLE_SECONDS;
        true
    }

    /// Follows the roster: admins leaving release their hold, an arriving
    /// player ends the reservation, a full lobby opens the reserved slot.
    pub fn observe(&mut self, lobby: &Lobby, now: u64) -> (Vec<Action>, Vec<String>) {
        self.admins.refresh(now);
        if lobby.lobby_name != self.lobby {
            *self = Self {
                admins: std::mem::take(&mut self.admins),
                lobby: lobby.lobby_name.clone(),
                ..Self::default()
            };
        }
        let mut actions = Vec::new();
        let mut lines = Vec::new();
        let here = |name: &str| humans(lobby).any(|p| p.name == name);
        if let Some(by) = &self.hold
            && !here(by)
        {
            lines.push(both(
                format!("{by} left: auto start is back on."),
                format!("{by} вышел: автостарт снова включён."),
            ));
            self.hold = None;
        }
        let Some(reserve) = &self.reserve else {
            return (actions, lines);
        };
        let (player, by, reserved) = (reserve.player.clone(), reserve.by.clone(), reserve.slot);
        let slot = lobby.players.iter().find(|p| p.slot == reserved);
        let arrived = humans(lobby).any(|p| same(&p.name, &player));
        let closed = slot.is_some_and(|p| p.slot_status == 1);
        if arrived || !here(&by) || slot.is_none() {
            lines.push(if arrived {
                both(
                    format!("{player} is here: the reservation is over."),
                    format!("{player} пришёл: резерв снят."),
                )
            } else {
                both(
                    format!("{by} left: the reserved slot is open again."),
                    format!("{by} вышел: зарезервированный слот снова открыт."),
                )
            });
            if closed {
                actions.push(Action::Open(reserved));
            }
            self.reserve = None;
            return (actions, lines);
        }
        let other_open = lobby
            .players
            .iter()
            .any(|p| p.slot != reserved && p.slot_status == 0 && playing(p));
        let intruder = slot
            .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
            .map(|p| p.slot);
        if let Some(intruder) = intruder {
            // Someone other than the reserved player took the reserved slot.
            if self.settled(now) {
                lines.push(both(
                    format!("This slot is reserved for {player}."),
                    format!("Этот слот зарезервирован для {player}."),
                ));
                actions.push(Action::Kick(intruder));
            }
        } else if closed && !other_open {
            // The lobby is otherwise full: open the slot so the reserved
            // player can join, and keep everyone else out of it.
            if self.settled(now) {
                lines.push(both(
                    format!("The lobby is full: the last slot is kept for {player}."),
                    format!("Лобби заполнено: последний слот держим для {player}."),
                ));
                actions.push(Action::Open(reserved));
            }
        } else if !closed && other_open && self.settled(now) {
            actions.push(Action::Close(reserved));
        }
        (actions, lines)
    }

    /// Admin chat commands. Returns the reply and the lobby edits to request.
    pub fn handle(
        &mut self,
        event: &Value,
        lobby: &Lobby,
        now: u64,
    ) -> Option<(String, Vec<Action>, String)> {
        if event["messageType"] != "ChatMessage" {
            return None;
        }
        let message = &event["payload"]["message"];
        if message["source"] != "gameChat"
            || message["channelName"]
                .as_str()
                .is_some_and(|name| name != lobby.lobby_name)
        {
            return None;
        }
        let content = message["content"].as_str()?.trim();
        let (keyword, argument) = content
            .split_once(char::is_whitespace)
            .unwrap_or((content, ""));
        let word = keyword.strip_prefix(['?', '!'])?.to_ascii_lowercase();
        if !matches!(
            word.as_str(),
            "kick" | "stop" | "go" | "reserve" | "open" | "admin"
        ) {
            return None;
        }
        let sender = message["sender"].as_str()?;
        let mut senders = humans(lobby).filter(|p| p.name == sender);
        let me = senders.next()?;
        if senders.next().is_some() {
            return None;
        }
        let activity = format!("{sender} · {word}");
        if !self.admins.contains(&me.name) {
            return Some((
                both(
                    "Only lobby admins can use this command.",
                    "Эта команда только для админов лобби.",
                ),
                vec![],
                activity,
            ));
        }
        let argument = argument.trim();
        let (reply, actions) = match word.as_str() {
            "admin" => (
                both(
                    "Admin: ?kick <name>, ?stop (hold the start), ?go, ?reserve <name>, ?open.",
                    "Админ: ?kick <имя>, ?stop (держать старт), ?go, ?reserve <имя>, ?open.",
                ),
                vec![],
            ),
            "kick" => self.kick(lobby, &me.name, argument),
            "stop" => {
                self.hold = Some(me.name.clone());
                (
                    both(
                        format!("{} stopped the start. ?go to allow it.", me.name),
                        format!("{} остановил старт. ?go — разрешить.", me.name),
                    ),
                    vec![],
                )
            }
            "go" => {
                self.hold = None;
                let waiting = self.reserve.as_ref().map(|r| r.player.clone());
                (
                    match waiting {
                        Some(player) => both(
                            format!("Start allowed. Still waiting for {player} (?open to stop)."),
                            format!("Старт разрешён. Ещё ждём {player} (?open — не ждать)."),
                        ),
                        None => both("Start allowed.", "Старт разрешён."),
                    },
                    vec![],
                )
            }
            "reserve" => self.reserve(lobby, &me.name, argument),
            _ => match self.reserve.take() {
                Some(reserve) => {
                    let closed = lobby
                        .players
                        .iter()
                        .any(|p| p.slot == reserve.slot && p.slot_status == 1);
                    (
                        both(
                            format!("Reservation for {} cancelled, slot open.", reserve.player),
                            format!("Резерв для {} снят, слот открыт.", reserve.player),
                        ),
                        if closed {
                            vec![Action::Open(reserve.slot)]
                        } else {
                            vec![]
                        },
                    )
                }
                None => (
                    both("No slot is reserved.", "Зарезервированных слотов нет."),
                    vec![],
                ),
            },
        };
        if !actions.is_empty() {
            self.settle_until = now + SETTLE_SECONDS;
        }
        Some((reply, actions, activity))
    }

    fn kick(&mut self, lobby: &Lobby, admin: &str, name: &str) -> (String, Vec<Action>) {
        if name.is_empty() {
            return (both("Use ?kick <name>.", "Пишите ?kick <имя>."), vec![]);
        }
        let mut found = humans(lobby).filter(|p| same(&p.name, name));
        let Some(target) = found.next() else {
            return (both("Player not found.", "Игрок не найден."), vec![]);
        };
        if found.next().is_some() {
            return (
                both(
                    "That name is ambiguous. Use the full BattleTag.",
                    "Таких имён несколько. Напишите полный BattleTag.",
                ),
                vec![],
            );
        }
        if target.name == admin || self.admins.contains(&target.name) {
            return (
                both(
                    "Lobby admins cannot be kicked.",
                    "Админов лобби кикать нельзя.",
                ),
                vec![],
            );
        }
        (
            both(
                format!("{admin} kicked {}.", target.name),
                format!("{admin} кикнул {}.", target.name),
            ),
            vec![Action::Kick(target.slot)],
        )
    }

    fn reserve(&mut self, lobby: &Lobby, admin: &str, name: &str) -> (String, Vec<Action>) {
        if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
            return (
                both("Use ?reserve <name>.", "Пишите ?reserve <имя>."),
                vec![],
            );
        }
        if let Some(reserve) = &self.reserve {
            return (
                both(
                    format!(
                        "A slot is already reserved for {}. ?open first.",
                        reserve.player
                    ),
                    format!(
                        "Слот уже зарезервирован для {}. Сначала ?open.",
                        reserve.player
                    ),
                ),
                vec![],
            );
        }
        if humans(lobby).any(|p| same(&p.name, name)) {
            return (
                both(
                    format!("{name} is already in the lobby."),
                    format!("{name} уже в лобби."),
                ),
                vec![],
            );
        }
        let Some(slot) = lobby
            .players
            .iter()
            .filter(|p| p.slot_status == 0 && playing(p))
            .map(|p| p.slot)
            .max()
        else {
            return (
                both(
                    "No free slot to reserve.",
                    "Нет свободного слота для резерва.",
                ),
                vec![],
            );
        };
        self.reserve = Some(Reserve {
            player: name.to_owned(),
            by: admin.to_owned(),
            slot,
        });
        (
            both(
                format!(
                    "Slot {} is reserved for {name}. The start waits for them; ?open cancels.",
                    slot + 1
                ),
                format!(
                    "Слот {} зарезервирован для {name}. Старт ждёт его; ?open — снять резерв.",
                    slot + 1
                ),
            ),
            vec![Action::Close(slot)],
        )
    }
}

#[cfg(test)]
#[path = "lobby_admin_tests.rs"]
mod tests;
