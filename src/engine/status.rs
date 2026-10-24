use super::*;

impl Engine {
    pub(crate) fn host_placement_ready(&self, now: u64) -> bool {
        self.owned_lobby(now)
            .and_then(Lobby::local)
            .is_some_and(|p| {
                p.slot_status == 2 && (self.config.host_mode != HostMode::Observer || p.is_observer)
            })
    }

    pub fn lobby_status(&self, now: u64) -> String {
        match self.phase {
            Phase::Starting(_) => {
                return "Native countdown or loading transition in progress.".into();
            }
            Phase::Countdown(until) => {
                return format!("Starting in {} s.", until.saturating_sub(now));
            }
            _ => {}
        }
        let Some(lobby) = self.owned_lobby(now) else {
            return "Waiting for a fresh lobby update.".into();
        };
        let humans = lobby.humans(self.config.count_host);
        let condition = if self.config.require_full {
            let open = lobby
                .players
                .iter()
                .filter(|p| p.slot_status == 0 && !p.is_observer && p.team != OBSERVER_TEAM)
                .count();
            format!("Auto start: full lobby; {open} open playing slot(s), {humans} human(s).")
        } else {
            format!(
                "Auto start: {humans}/{} humans; {} more needed.",
                self.config.min_players,
                (self.config.min_players as usize).saturating_sub(humans)
            )
        };
        let reason = if self.config.host_mode == HostMode::Observer
            && lobby.local().is_none_or(|p| !p.is_observer)
        {
            "Waiting for host observer placement.".into()
        } else if !self.ladder_allows_start(lobby) {
            if lobby.humans(true) < 4 {
                "Waiting for four human players before ladder preparation.".into()
            } else {
                "Waiting for ladder preparation to finish.".into()
            }
        } else if !self.config.auto_start {
            "Automatic start is off; waiting for a start vote or the host.".into()
        } else if now < self.retry_start_after {
            format!("Retrying start in {} s.", self.retry_start_after - now)
        } else if self.cancelled_roster.as_ref() == Some(&lobby.start_signature()) {
            "Previous start cancelled; waiting for a roster change or a new start vote.".into()
        } else if humans == 0 {
            "Waiting for human players.".into()
        } else {
            "Waiting for the start condition and Warcraft readiness.".into()
        };
        format!("{condition} {reason}")
    }
}
