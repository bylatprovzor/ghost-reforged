//! Prepare one coherent four-player snapshot before any start path is enabled.
use crate::{
    engine::{Engine, Phase},
    ladder::{self, Packet},
    protocol::{Command, Lobby, Player},
};
use serde_json::json;
use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

#[derive(Default, PartialEq)]
enum Stage {
    #[default]
    Waiting,
    Normalize,
    Fetch,
    Write(usize),
    Commit,
    Ready,
    Unavailable,
}

struct Job {
    key: String,
    rx: Receiver<Result<ladder::Prepared, ladder::Error>>,
}

#[derive(Default)]
pub(super) struct Seed {
    match_id: Option<String>,
    key: String,
    stage: Stage,
    job: Option<Job>,
    packet: Option<Packet>,
    sequence: u64,
    awaiting: Option<(u64, u64)>,
    written: bool,
    // SetComputerSlot is sent on its own: on 3.0.1 it recreates the AI after
    // later edits in the same batch and resets race and handicap.
    slot_sent: bool,
    retries: u8,
    notice: Option<String>,
    replies: std::collections::VecDeque<String>,
    reply_at: u64,
    began: u64,
    expires_at: Option<u64>,
    room_awaiting: Option<(u64, u64)>,
}

fn humans(lobby: &Lobby) -> Option<[&Player; 4]> {
    let players: Vec<_> = lobby
        .players
        .iter()
        .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_observer)
        .collect();
    if players.len() != 4 {
        return None;
    }
    let mut rows = players;
    rows.sort_by_key(|p| p.team);
    if rows
        .iter()
        .enumerate()
        .any(|(i, p)| p.team as usize != i || !p.name.contains('#'))
    {
        return None;
    }
    rows.try_into().ok()
}

fn roster_key(lobby: &Lobby) -> Option<String> {
    let rows = humans(lobby)?;
    Some(format!(
        "{}|{}|{}|{}",
        ladder::uses_mmr_packet(&lobby.map_data.map_name),
        lobby.map_data.map_path,
        lobby.lobby_name,
        rows.iter()
            .map(|p| format!(
                "{}:{:?}:{}:{:?}",
                p.team,
                p.id,
                p.name,
                ladder::Region::from_player(p)
            ))
            .collect::<Vec<_>>()
            .join("|")
    ))
}

fn by_color(lobby: &Lobby, color: u8) -> Option<&Player> {
    let mut rows = lobby
        .players
        .iter()
        .filter(|p| p.slot_status == 2 && !p.is_observer && p.color == Some(color));
    let row = rows.next()?;
    rows.next().is_none().then_some(row)
}

fn four_teams(lobby: &Lobby) -> bool {
    (0..16).all(|slot| {
        lobby
            .players
            .iter()
            .any(|p| p.slot == slot && p.team == slot % 4 && !p.is_observer)
    })
}

fn computer_row<'a>(lobby: &'a Lobby, packet: &Packet, index: usize) -> Option<&'a Player> {
    if matches!(packet.handicaps[0], 43..=46) {
        lobby.players.iter().find(|p| {
            p.slot as usize == index + 4 && p.slot_type == 1 && p.slot_status == 2 && !p.is_observer
        })
    } else {
        by_color(lobby, packet.computers[index].color)
    }
}

impl Seed {
    pub(super) fn reset(&mut self) {
        // Keep a single bounded HTTP job even if a roster changes repeatedly.
        let job = self.job.take();
        *self = Self::default();
        self.job = job;
    }

    pub(super) fn notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    pub(super) fn observe(&mut self) {
        self.sequence += 1;
    }

    pub(super) fn ready(&self, lobby: &Lobby) -> bool {
        self.replies.is_empty() && self.snapshot_ready(lobby)
    }

    pub(super) fn confirmed_match(&self, lobby: &Lobby) -> Option<&str> {
        (self.stage == Stage::Ready && self.snapshot_ready(lobby))
            .then_some(self.match_id.as_deref())
            .flatten()
    }

    fn snapshot_ready(&self, lobby: &Lobby) -> bool {
        if !ladder::matches_map(&lobby.map_data.map_name) {
            return true;
        }
        if roster_key(lobby).as_ref() != Some(&self.key) {
            return false;
        }
        let Some(rows) = humans(lobby) else {
            return false;
        };
        if four_teams(lobby)
            && (rows.iter().enumerate().any(|(i, p)| p.slot as usize != i)
                || !(4..16).all(|slot| {
                    lobby
                        .players
                        .iter()
                        .any(|p| p.slot == slot && p.slot_type == 1 && p.slot_status == 2)
                }))
        {
            return false;
        }
        if self.stage == Stage::Unavailable {
            return rows.iter().all(|p| p.handicap == Some(100));
        }
        if self.stage != Stage::Ready {
            return false;
        }
        if self.session_expired() {
            return false;
        }
        let Some(packet) = &self.packet else {
            return false;
        };
        rows.iter()
            .enumerate()
            .all(|(i, p)| p.color == Some(i as u8) && p.handicap == Some(packet.handicaps[i]))
            && self.body_matches(lobby)
    }

    fn session_expired(&self) -> bool {
        self.expires_at.is_some_and(|expiry| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(true, |now| expiry <= now.as_secs().saturating_add(3600))
        })
    }

    fn body_matches(&self, lobby: &Lobby) -> bool {
        self.packet.as_ref().is_some_and(|packet| {
            packet.computers.iter().enumerate().all(|(index, s)| {
                computer_row(lobby, packet, index).is_some_and(|p| {
                    p.slot_type == 1
                        && p.color == Some(s.color)
                        && p.team == s.team
                        && p.handicap == Some(s.handicap)
                        && p.race == Some(s.race)
                })
            })
        })
    }

    fn fail(&mut self, reason: impl ToString) {
        self.match_id = None;
        self.stage = Stage::Unavailable;
        self.packet = None;
        self.awaiting = None;
        self.notice = Some(reason.to_string());
        self.replies.clear();
        self.replies.push_back(
            "Ladder unavailable. This game is unranked; the ladder intro will be skipped.".into(),
        );
    }

    fn sent(&mut self, now: u64, mut commands: Vec<Command>) -> Vec<Command> {
        self.awaiting = Some((self.sequence, now));
        commands.push(Command::empty("SendGameLobbySetup"));
        commands
    }

    pub(super) fn advance(
        &mut self,
        engine: &Engine,
        now: u64,
        quiet: bool,
        identity_ok: bool,
    ) -> Vec<Command> {
        if !matches!(engine.phase, Phase::Lobby | Phase::Countdown(_)) {
            return Vec::new();
        }
        let Some(lobby) = engine.owned_lobby(now) else {
            return Vec::new();
        };
        if !identity_ok || !ladder::matches_map(&lobby.map_data.map_name) {
            return Vec::new();
        }
        // Unlocked custom forces retain their seats but Warcraft opens the AI
        // slots. Fill only those empty reserved seats, before accepting a roster.
        if four_teams(lobby) {
            let empty: Vec<_> = lobby
                .players
                .iter()
                .filter(|p| (4..16).contains(&p.slot) && p.slot_status != 2)
                .collect();
            if !empty.is_empty() {
                if !quiet {
                    return Vec::new();
                }
                if let Some((sequence, at)) = self.room_awaiting {
                    if now.saturating_sub(at) >= 8 {
                        self.notice = Some(
                            "Warcraft did not restore the ranked map's computer slots.".into(),
                        );
                        return Vec::new();
                    }
                    if self.sequence <= sequence {
                        return Vec::new();
                    }
                }
                let began = self.room_awaiting.map_or(now, |(_, at)| at);
                self.room_awaiting = Some((self.sequence, began));
                let mut commands: Vec<_> = empty
                    .into_iter()
                    .map(|p| Command::new("SetComputerSlot", json!({"slot":p.slot,"difficulty":1})))
                    .collect();
                commands.push(Command::empty("SendGameLobbySetup"));
                return commands;
            }
            self.room_awaiting = None;
        }
        let Some(key) = roster_key(lobby) else {
            self.key.clear();
            self.stage = Stage::Waiting;
            return Vec::new();
        };
        if key != self.key {
            self.match_id = None;
            self.began = now;
            self.key = key;
            self.stage = Stage::Normalize;
            self.packet = None;
            self.awaiting = None;
            self.replies.clear();
            self.written = false;
            self.slot_sent = false;
            self.retries = 0;
            self.expires_at = None;
        }
        if !matches!(self.stage, Stage::Ready | Stage::Unavailable)
            && now.saturating_sub(self.began) >= 120
        {
            self.fail("Ladder preparation timed out; skipping the intro.");
        }
        if let Some((sequence, at)) = self.awaiting {
            if now.saturating_sub(at) >= 8 {
                self.fail("Warcraft did not confirm the ladder lobby settings.");
            } else if !quiet || self.sequence <= sequence {
                return Vec::new();
            } else {
                self.awaiting = None;
            }
        }
        if !quiet {
            return Vec::new();
        }
        if self.stage == Stage::Ready && self.session_expired() {
            self.began = now;
            self.stage = Stage::Fetch;
            self.packet = None;
            self.replies.clear();
        } else if self.stage == Stage::Ready && !self.snapshot_ready(lobby) {
            self.began = now;
            self.stage = Stage::Write(0);
            self.written = false;
            self.slot_sent = false;
            self.retries = 0;
            self.replies.clear();
        }
        // Invalidate the marker before writing any fields, including on failure.
        if matches!(
            self.stage,
            Stage::Normalize | Stage::Fetch | Stage::Write(_) | Stage::Unavailable
        ) {
            let row = humans(lobby)
                .unwrap()
                .into_iter()
                .find(|p| p.handicap != Some(100));
            if let Some(row) = row {
                return self.sent(
                    now,
                    vec![Command::new(
                        "SetHandicap",
                        json!({"slot":row.slot,"handicap":100}),
                    )],
                );
            }
        }
        match self.stage {
            Stage::Normalize => self.normalize(lobby, now),
            Stage::Fetch => {
                if let Some(job) = &self.job {
                    match job.rx.try_recv() {
                        Ok(result) => {
                            let job_key = job.key.clone();
                            self.job = None;
                            if job_key != self.key {
                                return Vec::new();
                            }
                            match result {
                                Ok(prepared) => {
                                    self.match_id = prepared.match_id;
                                    self.expires_at = prepared.expires_at;
                                    self.packet = Some(prepared.packet);
                                    self.stage = Stage::Write(0);
                                }
                                Err(error) => self.fail(error),
                            }
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let current_job = job.key == self.key;
                            self.job = None;
                            if current_job {
                                self.fail("Ladder request failed.");
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                } else {
                    let tags = humans(lobby).unwrap().map(|p| p.name.clone());
                    let regions = humans(lobby).unwrap().map(ladder::Region::from_player);
                    let map_name = lobby.map_data.map_name.clone();
                    let (tx, rx) = mpsc::channel();
                    self.job = Some(Job {
                        key: self.key.clone(),
                        rx,
                    });
                    thread::spawn(move || {
                        let _ = tx.send(ladder::prepare_for_map(&tags, &regions, &map_name));
                    });
                }
                Vec::new()
            }
            Stage::Write(index) => {
                if index == 12 {
                    if !self.body_matches(lobby) {
                        self.fail("Ladder packet was changed before confirmation.");
                    } else {
                        self.stage = Stage::Commit;
                    }
                    return Vec::new();
                }
                let s = self.packet.as_ref().unwrap().computers[index].clone();
                let packet = self.packet.as_ref().unwrap();
                let Some(p) = computer_row(lobby, packet, index).filter(|p| p.slot_type == 1)
                else {
                    self.fail("Ladder computer slot is missing.");
                    return Vec::new();
                };
                let legacy = matches!(packet.handicaps[0], 43..=46);
                let fields = |slot: u8| {
                    let mut commands = vec![
                        Command::new("SetRace", json!({"slot":slot,"raceId":s.race})),
                        Command::new("SetHandicap", json!({"slot":slot,"handicap":s.handicap})),
                    ];
                    if !legacy {
                        commands.push(Command::new(
                            "SetTeam",
                            json!({"slot":slot,"team":s.team}),
                        ));
                    }
                    commands
                };
                if self.written {
                    if p.handicap != Some(s.handicap) || p.race != Some(s.race) || p.team != s.team
                    {
                        if self.retries < 3 {
                            // A delayed AI recreation can reset the fields; write them again.
                            self.retries += 1;
                            let commands = fields(p.slot);
                            return self.sent(now, commands);
                        }
                        self.fail("Warcraft rejected a ladder packet field.");
                    } else if p.color != Some(s.color) {
                        // SetComputerSlot recreates the AI and resets its color.
                        // Apply the payload color only after that acknowledgement.
                        return self.set_color(lobby, p, s.color, now);
                    } else {
                        self.stage = Stage::Write(index + 1);
                        self.written = false;
                        self.slot_sent = false;
                        self.retries = 0;
                    }
                    return Vec::new();
                }
                if !self.slot_sent {
                    self.slot_sent = true;
                    let slot = p.slot;
                    return self.sent(
                        now,
                        vec![Command::new(
                            "SetComputerSlot",
                            json!({"slot":slot,"difficulty":s.difficulty}),
                        )],
                    );
                }
                self.written = true;
                // Team changes can reorder rows. Resolve the next color only
                // after receiving a fresh snapshot of this completed edit.
                let commands = fields(p.slot);
                self.sent(now, commands)
            }
            Stage::Commit => {
                if !self.body_matches(lobby) {
                    self.fail("Ladder packet changed before start.");
                    return Vec::new();
                }
                let packet = self.packet.as_ref().unwrap();
                // Write the marker last. A half-written packet never shows cards.
                for i in [2, 3, 1, 0] {
                    let p = humans(lobby).unwrap()[i];
                    if p.handicap != Some(packet.handicaps[i]) {
                        return self.sent(
                            now,
                            vec![Command::new(
                                "SetHandicap",
                                json!({"slot":p.slot,"handicap":packet.handicaps[i]}),
                            )],
                        );
                    }
                }
                self.stage = Stage::Ready;
                self.notice = Some(
                    if matches!(packet.handicaps[0], 44 | 46)
                        || (packet.handicaps[0] == 47 && packet.handicaps[1] == 84)
                    {
                        "Ladder profiles and result ticket confirmed for all four players."
                    } else {
                        "Ladder preview confirmed; result reporting is disabled."
                    }
                    .into(),
                );
                // Player statistics belong to the join welcome and ?stats.
                Vec::new()
            }
            Stage::Ready | Stage::Unavailable if now >= self.reply_at => {
                self.reply_at = now + 1;
                self.replies
                    .pop_front()
                    .map(|content| {
                        vec![Command::new(
                            "SendGameChatMessage",
                            json!({"content":content}),
                        )]
                    })
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn normalize(&mut self, lobby: &Lobby, now: u64) -> Vec<Command> {
        let active: Vec<_> = lobby
            .players
            .iter()
            .filter(|p| p.slot_status == 2 && !p.is_observer)
            .collect();
        if active.len() != 16 || active.iter().filter(|p| p.slot_type == 1).count() != 12 {
            self.fail("The ranked map requires its twelve computer slots.");
            return Vec::new();
        }
        if four_teams(lobby) {
            if humans(lobby)
                .unwrap()
                .iter()
                .enumerate()
                .any(|(i, p)| p.slot as usize != i)
            {
                self.fail("Ranked human players must occupy the four original human seats.");
                return Vec::new();
            }
            for color in 0..16 {
                let target = active.iter().find(|p| p.slot == color).unwrap();
                if target.color != Some(color) {
                    return self.set_color(lobby, target, color, now);
                }
            }
            self.stage = Stage::Fetch;
            return Vec::new();
        }
        // A newly created map uses one team per role. A reconnect can reuse the
        // already-normalized colors if computer teams contain an earlier packet.
        let initial = (0..16).all(|team| active.iter().filter(|p| p.team == team).count() == 1);
        if !initial {
            if (4..16).all(|c| by_color(lobby, c).is_some_and(|p| p.slot_type == 1))
                && humans(lobby)
                    .unwrap()
                    .iter()
                    .enumerate()
                    .all(|(i, p)| p.color == Some(i as u8))
            {
                self.stage = Stage::Fetch;
            } else {
                self.fail("Ranked lobby positions were changed; profiles were not seeded.");
            }
            return Vec::new();
        }
        for color in 0..16 {
            let target = active.iter().find(|p| p.team == color).unwrap();
            if target.color == Some(color) {
                continue;
            }
            if let Some(occupied) = active.iter().find(|p| p.color == Some(color)) {
                let Some(free) = (0..24).find(|c| active.iter().all(|p| p.color != Some(*c)))
                else {
                    self.fail("No spare lobby color for the ranked map.");
                    return Vec::new();
                };
                return self.sent(
                    now,
                    vec![Command::new(
                        "SetColor",
                        json!({"slot":occupied.slot,"color":free}),
                    )],
                );
            }
            return self.sent(
                now,
                vec![Command::new(
                    "SetColor",
                    json!({"slot":target.slot,"color":color}),
                )],
            );
        }
        self.stage = Stage::Fetch;
        Vec::new()
    }

    fn set_color(&mut self, lobby: &Lobby, target: &Player, color: u8, now: u64) -> Vec<Command> {
        let active: Vec<_> = lobby
            .players
            .iter()
            .filter(|p| p.slot_status == 2 && !p.is_observer)
            .collect();
        let (slot, next) = if let Some(occupied) = active.iter().find(|p| p.color == Some(color)) {
            let Some(free) = (4..24).find(|c| active.iter().all(|p| p.color != Some(*c))) else {
                self.fail("No spare lobby color for the ranked map.");
                return Vec::new();
            };
            (occupied.slot, free)
        } else {
            (target.slot, color)
        };
        self.sent(
            now,
            vec![Command::new("SetColor", json!({"slot":slot,"color":next}))],
        )
    }
}

#[cfg(test)]
#[path = "ladder_tests.rs"]
mod tests;
