use crate::{commands::CommandSettings, engine::Engine, welcome::Welcome};

/// Settings that the controller can change without replacing the current lobby.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSettings {
    pub headless: bool,
    pub minimum: u8,
    pub full: bool,
    pub rehost: bool,
    pub rehost_delay: u64,
    pub empty_refresh: u64,
    pub commands: CommandSettings,
    pub welcome: Welcome,
}

impl LiveSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=23).contains(&self.minimum)
            || !(5..=3600).contains(&self.rehost_delay)
            || self.empty_refresh > 86400
        {
            return Err("Hosting settings are outside supported bounds.".into());
        }
        self.welcome.validate()
    }

    pub(super) fn apply(&self, engine: &mut Engine, now: u64) -> Result<(), String> {
        self.validate()?;
        engine.config.headless = self.headless;
        engine.update_start_rule(self.minimum, self.full);
        engine.update_rehost(self.rehost, self.rehost_delay, now);
        engine.config.empty_rehost_seconds = self.empty_refresh;
        engine.config.commands = self.commands.clone();
        engine.config.welcome = self.welcome.clone();
        Ok(())
    }
}
