//! Prepare one coherent four-player snapshot before any start path is enabled.
use crate::{
    engine::{Engine, Phase},
    ladder::{self, Packet, Profile},
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
    rx: Receiver<Result<[Profile; 4], ladder::Error>>,
}

#[derive(Default)]
pub(super) struct Seed {
    key: String,
    stage: Stage,
    job: Option<Job>,
    packet: Option<Packet>,
    profiles: Option<[Profile; 4]>,
    sequence: u64,
    awaiting: Option<(u64, u64)>,
    written: bool,
    notice: Option<String>,
    replies: std::collections::VecDeque<String>,
    reply_at: u64,
    began: u64,
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
        "{}|{}|{}",
        lobby.map_data.map_path,
        lobby.lobby_name,
        rows.iter()
            .map(|p| format!("{}:{:?}:{}", p.team, p.id, p.name))
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
        if self.stage == Stage::Unavailable {
            return rows.iter().all(|p| p.handicap == Some(100));
        }
        if self.stage != Stage::Ready {
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

    fn body_matches(&self, lobby: &Lobby) -> bool {
        self.packet.as_ref().is_some_and(|packet| {
            packet.computers.iter().all(|s| {
                by_color(lobby, s.color).is_some_and(|p| {
                    p.slot_type == 1
                        && p.team == s.team
                        && p.handicap == Some(s.handicap)
                        && p.race == Some(s.race)
                })
            })
        })
    }

    fn fail(&mut self, reason: impl ToString) {
        self.stage = Stage::Unavailable;
        self.packet = None;
        self.awaiting = None;
        self.notice = Some(reason.to_string());
        self.replies.clear();
        self.replies.push_back(
            "Ladder statistics unavailable. The game will start without the ladder intro.".into(),
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
        let Some(key) = roster_key(lobby) else {
            self.key.clear();
            self.stage = Stage::Waiting;
            return Vec::new();
        };
        if key != self.key {
            self.began = now;
            self.key = key;
            self.stage = Stage::Normalize;
            self.packet = None;
            self.profiles = None;
            self.awaiting = None;
            self.replies.clear();
            self.written = false;
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
        if self.stage == Stage::Ready && !self.snapshot_ready(lobby) {
            self.began = now;
            self.stage = Stage::Write(0);
            self.written = false;
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
                            match result.and_then(|profiles| {
                                Packet::encode(&profiles).map(|packet| (profiles, packet))
                            }) {
                                Ok((profiles, packet)) => {
                                    self.profiles = Some(profiles);
                                    self.packet = Some(packet);
                                    self.stage = Stage::Write(0);
                                }
                                Err(error) => self.fail(error),
                            }
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            self.job = None;
                            self.fail("Ladder request failed.");
                        }
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                } else {
                    let tags = humans(lobby).unwrap().map(|p| p.name.clone());
                    let (tx, rx) = mpsc::channel();
                    self.job = Some(Job {
                        key: self.key.clone(),
                        rx,
                    });
                    thread::spawn(move || {
                        let _ = tx.send(ladder::fetch(&tags));
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
                let Some(p) = by_color(lobby, s.color).filter(|p| p.slot_type == 1) else {
                    self.fail("Ladder computer slot is missing.");
                    return Vec::new();
                };
                if self.written {
                    if p.handicap != Some(s.handicap) || p.race != Some(s.race) || p.team != s.team
                    {
                        self.fail("Warcraft rejected a ladder packet field.");
                    } else {
                        self.stage = Stage::Write(index + 1);
                        self.written = false;
                    }
                    return Vec::new();
                }
                self.written = true;
                // Team changes can reorder rows. Resolve the next color only
                // after receiving a fresh snapshot of this completed edit.
                self.sent(
                    now,
                    vec![
                        Command::new(
                            "SetComputerSlot",
                            json!({"slot":p.slot,"difficulty":s.difficulty}),
                        ),
                        Command::new("SetRace", json!({"slot":p.slot,"raceId":s.race})),
                        Command::new("SetHandicap", json!({"slot":p.slot,"handicap":s.handicap})),
                        Command::new("SetTeam", json!({"slot":p.slot,"team":s.team})),
                    ],
                )
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
                self.notice = Some("Ladder profiles confirmed for all four players.".into());
                self.replies.extend(
                    self.profiles
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(Profile::announcement),
                );
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
}

#[cfg(test)]
#[path = "ladder_tests.rs"]
mod tests;
