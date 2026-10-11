use super::{Slot, Snapshot};
use crate::{
    engine::{Engine, Phase},
    protocol::{Command, Lobby},
};
use serde_json::json;

#[derive(Debug, Clone, PartialEq)]
pub struct KickRequest {
    lobby: String,
    slot: u8,
    id: i64,
    name: String,
}

impl Snapshot {
    pub fn kick_target(&self, slot: u8) -> Option<KickRequest> {
        let player = self.slots.iter().find(|p| p.number == slot)?;
        (self.can_edit_slots && player.kickable()).then(|| KickRequest {
            lobby: self.name.clone(),
            slot,
            id: player.id.unwrap(),
            name: player.name.clone(),
        })
    }
}

impl Slot {
    pub fn kickable(&self) -> bool {
        self.status == 2
            && !self.computer
            && !self.own
            && self.id.is_some_and(|id| id >= 0 && id != 255)
            && !self.name.trim().is_empty()
    }
}

impl KickRequest {
    fn matches(&self, lobby: &Lobby) -> bool {
        lobby.is_host
            && lobby
                .local()
                .is_some_and(|p| p.slot_status == 2 && p.slot_type == 0)
            && lobby.lobby_name == self.lobby
            && lobby.players.iter().any(|p| {
                p.slot == self.slot
                    && p.id == Some(self.id)
                    && p.name == self.name
                    && p.slot_status == 2
                    && p.slot_type == 0
                    && !p.is_self
            })
    }
}

#[derive(Default)]
pub(super) struct RefreshGate {
    requested: bool,
    observed: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum Refresh {
    Wait,
    Request,
    Confirm,
}

impl RefreshGate {
    pub fn advance(&mut self, roster: bool, idle: bool, alias_ready: bool) -> Refresh {
        if !self.requested {
            if idle && alias_ready {
                self.requested = true;
                return Refresh::Request;
            }
        } else {
            self.observed |= roster;
            if self.observed && idle && alias_ready {
                return Refresh::Confirm;
            }
        }
        Refresh::Wait
    }
}

#[derive(Default)]
pub(super) struct PendingKick(Option<(KickRequest, u64, RefreshGate)>);

impl PendingKick {
    pub fn request(&mut self, target: KickRequest, engine: &Engine, now: u64) -> Option<()> {
        if engine.phase != Phase::Lobby
            || !engine.owned_lobby(now).is_some_and(|l| target.matches(l))
        {
            return None;
        }
        self.0 = Some((target, now, RefreshGate::default()));
        Some(())
    }

    pub fn confirm(
        &mut self,
        engine: &Engine,
        now: u64,
        fresh_roster: bool,
        alias_ready: bool,
        input_idle: bool,
    ) -> Option<(Option<Command>, Option<String>)> {
        let (target, requested, gate) = self.0.as_mut()?;
        if now.saturating_sub(*requested) >= 5
            || engine.phase != Phase::Lobby
            || !engine.owned_lobby(now).is_some_and(|l| target.matches(l))
        {
            let name = target.name.clone();
            self.0 = None;
            return Some((None, Some(format!("Kick cancelled · {name}"))));
        }
        match gate.advance(fresh_roster, input_idle, alias_ready) {
            Refresh::Wait => return None,
            Refresh::Request => return Some((Some(Command::empty("SendGameLobbySetup")), None)),
            Refresh::Confirm => {}
        }
        let (target, _, _) = self.0.take()?;
        if engine.owned_lobby(now).is_some_and(|l| target.matches(l)) {
            Some((
                Some(Command::new(
                    "KickPlayerFromGameLobby",
                    json!({"slot":target.slot}),
                )),
                Some(format!("Kick requested · {}", target.name)),
            ))
        } else {
            Some((None, Some(format!("Kick cancelled · {}", target.name))))
        }
    }
}

pub(super) fn invalidate_vote(
    pending: &mut Option<(crate::commands::KickTarget, u64, RefreshGate)>,
    engine: &Engine,
    now: u64,
) {
    if pending.as_ref().is_some_and(|(target, requested, _)| {
        now.saturating_sub(*requested) >= 5
            || !matches!(engine.phase, Phase::Lobby | Phase::Countdown(_))
            || engine
                .owned_lobby(now)
                .is_none_or(|lobby| target.confirm(lobby).is_none())
    }) {
        *pending = None;
    }
}
