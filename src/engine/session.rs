use super::*;

impl Engine {
    pub(super) fn initialize_provider(&mut self, now: u64) -> Vec<Command> {
        self.phase = Phase::Initializing(now);
        // A newly launched client's menu initializes multiplayer after its
        // authentication callback. Keep one owner of that startup transition.
        if self.native_initialization {
            Vec::new()
        } else {
            vec![Command::empty("InitializeNetProvider")]
        }
    }

    pub(super) fn authentication(&mut self, event: &Value, now: u64) -> Option<Vec<Command>> {
        if event["messageType"] != "UpdateUserInfo" {
            return None;
        }
        match event["payload"]["user"]["isAuthenticated"].as_bool() {
            Some(false) => self.authenticated_initialization = false,
            Some(true) if self.awaiting_session() && !self.authenticated_initialization => {
                self.authenticated_initialization = true;
                // Native online readiness can arrive before the user callback
                // while LOGIN_DOORS still blocks creation. Do not reset that
                // already initialized provider when authentication catches up.
                if self.provider_ready && !self.initialization_required {
                    return Some(Vec::new());
                }
                return Some(self.initialize_provider(now));
            }
            _ => {}
        }
        None
    }
}
