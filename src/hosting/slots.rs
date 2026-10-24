use super::{
    Slot, Snapshot,
    kick::{Refresh, RefreshGate},
};
use crate::{
    engine::{Engine, Phase},
    protocol::{Command, Lobby, OBSERVER_TEAM},
};
use serde_json::json;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotAction {
    Open,
    Close,
    Observer,
    Computer,
}

impl SlotAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Close => "Close",
            Self::Observer => "Move to observers",
            Self::Computer => "Add computer",
        }
    }

    fn allowed(self, slot: &Slot, observer_space: bool) -> bool {
        if slot.own {
            return false;
        }
        match self {
            Self::Open => slot.change_type && (slot.status == 1 || slot.computer),
            Self::Close => slot.change_type && (slot.status == 0 || slot.computer),
            Self::Computer => !slot.observer && slot.change_type && matches!(slot.status, 0 | 1),
            Self::Observer => {
                slot.change_team
                    && !slot.observer
                    && !slot.computer
                    && slot.status == 2
                    && observer_space
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotRequest {
    lobby: String,
    target: Slot,
    action: SlotAction,
}

impl Snapshot {
    pub fn slot_actions(&self, number: u8) -> Vec<(SlotAction, SlotRequest)> {
        let Some(slot) = self.slots.iter().find(|s| s.number == number) else {
            return vec![];
        };
        let observer_space = self.slots.iter().any(|s| s.observer && s.status == 0);
        [
            SlotAction::Open,
            SlotAction::Close,
            SlotAction::Observer,
            SlotAction::Computer,
        ]
        .into_iter()
        .filter(|action| self.can_edit_slots && action.allowed(slot, observer_space))
        .map(|action| {
            (
                action,
                SlotRequest {
                    lobby: self.name.clone(),
                    target: slot.clone(),
                    action,
                },
            )
        })
        .collect()
    }
}

impl SlotRequest {
    pub(super) fn rejected_notice(&self) -> String {
        format!(
            "Slot {} · {} · not applied",
            self.target.number + 1,
            self.action.label()
        )
    }

    fn matches(&self, lobby: &Lobby) -> bool {
        let current = super::summarize(lobby, false, true);
        current.name == self.lobby
            && current
                .slot_actions(self.target.number)
                .iter()
                .any(|(_, request)| request == self)
    }

    fn command(&self) -> Command {
        let slot = self.target.number;
        match self.action {
            SlotAction::Open => Command::new("OpenSlot", json!({"slot":slot})),
            SlotAction::Close => Command::new("CloseSlot", json!({"slot":slot})),
            SlotAction::Observer => {
                Command::new("SetTeam", json!({"slot":slot,"team":OBSERVER_TEAM}))
            }
            SlotAction::Computer => {
                Command::new("SetComputerSlot", json!({"slot":slot,"difficulty":1}))
            }
        }
    }

    fn applied(&self, lobby: &Lobby) -> bool {
        if lobby.lobby_name != self.lobby {
            return false;
        }
        lobby.players.iter().any(|p| match self.action {
            SlotAction::Observer => {
                p.id == self.target.id
                    && p.name == self.target.name
                    && p.slot_status == 2
                    && (p.is_observer || p.team == OBSERVER_TEAM)
            }
            SlotAction::Open => p.slot == self.target.number && p.slot_status == 0,
            SlotAction::Close => p.slot == self.target.number && p.slot_status == 1,
            SlotAction::Computer => {
                p.slot == self.target.number && p.slot_status == 2 && p.slot_type == 1
            }
        })
    }
}

#[derive(Default)]
pub(super) struct PendingSlot {
    request: Option<(SlotRequest, u64, RefreshGate)>,
    queued: VecDeque<(SlotRequest, u64)>,
    sent: bool,
}

impl PendingSlot {
    pub fn close_empty_observer(&mut self, engine: &Engine, now: u64) {
        if self.request.is_some()
            || !self.queued.is_empty()
            || engine.phase != Phase::Lobby
            || !engine.host_placement_ready(now)
        {
            return;
        }
        let Some(lobby) = engine.owned_lobby(now) else {
            return;
        };
        let snapshot = super::summarize(lobby, false, true);
        let request = snapshot
            .slots
            .iter()
            .filter(|slot| slot.observer && !slot.own && slot.status == 0)
            .flat_map(|slot| snapshot.slot_actions(slot.number))
            .find(|(action, _)| *action == SlotAction::Close);
        if let Some((_, request)) = request {
            // Recheck the empty slot against a fresh roster before closing it.
            self.request(request, engine, now);
        }
    }

    pub fn request(&mut self, request: SlotRequest, engine: &Engine, now: u64) -> bool {
        if engine.phase != Phase::Lobby
            || !engine.host_placement_ready(now)
            || !engine.owned_lobby(now).is_some_and(|l| request.matches(l))
        {
            return false;
        }
        if self.request.is_none() && self.queued.is_empty() {
            self.request = Some((request, now, RefreshGate::default()));
            self.sent = false;
        } else {
            if self.queued.len() >= 24 {
                return false;
            }
            self.queued.push_back((request, now));
        }
        true
    }

    pub fn advance(
        &mut self,
        engine: &Engine,
        now: u64,
        roster: bool,
        idle: bool,
        alias: bool,
    ) -> (Vec<Command>, Option<String>) {
        if self.request.is_none()
            && let Some((request, queued_at)) = self.queued.pop_front()
        {
            // Manual edits take priority over further automatic closures. Never
            // apply a queued snapshot to a changed player, lobby or game phase.
            if now.saturating_sub(queued_at) >= 30
                || engine.phase != Phase::Lobby
                || !engine.host_placement_ready(now)
                || !engine.owned_lobby(now).is_some_and(|l| request.matches(l))
            {
                return (vec![], Some(request.rejected_notice()));
            }
            self.request = Some((request, now, RefreshGate::default()));
            self.sent = false;
        }
        let Some((request, started, gate)) = self.request.as_mut() else {
            return (vec![], None);
        };
        let lobby = engine.owned_lobby(now);
        let applied = self.sent && roster && lobby.is_some_and(|l| request.applied(l));
        let invalid = now.saturating_sub(*started) >= 6
            || engine.phase != Phase::Lobby
            || !engine.host_placement_ready(now)
            || (!self.sent && !lobby.is_some_and(|l| request.matches(l)));
        if applied || invalid {
            let line = format!(
                "Slot {} · {} · {}",
                request.target.number + 1,
                request.action.label(),
                if applied { "applied" } else { "not applied" }
            );
            self.request = None;
            return (vec![], Some(line));
        }
        if !self.sent {
            match gate.advance(roster, idle, alias) {
                Refresh::Request => return (vec![Command::empty("SendGameLobbySetup")], None),
                Refresh::Confirm => {
                    self.sent = true;
                    *started = now;
                    return (
                        vec![request.command(), Command::empty("SendGameLobbySetup")],
                        None,
                    );
                }
                Refresh::Wait => {}
            }
        }
        (vec![], None)
    }
}
