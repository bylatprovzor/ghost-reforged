use super::*;

impl Engine {
    pub(super) fn finish_game(&mut self, now: u64) {
        self.completed = self.completed.saturating_add(1);
        if self.config.max_games > 0 && self.completed >= self.config.max_games {
            self.stop("Configured game limit reached");
        } else {
            self.map_index = (self.map_index + 1) % self.config.maps.len();
            self.release(now);
        }
    }

    pub fn client_stopped(&mut self, now: u64) {
        if self.phase == Phase::Restarting {
            self.finish_game(now);
        }
    }

    pub(crate) fn retry_incomplete_lobby(&mut self) {
        // A failed creation does not consume a completed game or a second reservation.
        let config = Config {
            first_number: self.number,
            ..self.config.clone()
        };
        let completed = self.completed;
        let map_index = self.map_index;
        *self = Self::new(config);
        self.completed = completed;
        self.map_index = map_index;
    }

    pub fn expect_client_initialization(&mut self) {
        self.native_initialization = true;
        self.authenticated_initialization = false;
        self.initialization_required = true;
        self.provider_ready = false;
    }

    pub(super) fn release(&mut self, now: u64) {
        self.provider_ready = false;
        self.start = None;
        self.countdown_locked = false;
        self.start_confirmed = false;
        self.cancelled_roster = None;
        let Some(next) = self.number.checked_add(1) else {
            self.stop("Lobby number exhausted");
            return;
        };
        self.number = next;
        self.owned = false;
        self.latest = None;
        self.empty_since = None;
        self.move_sent = None;
        self.phase = Phase::Cooldown(now + self.config.rehost_delay_seconds);
    }
}
