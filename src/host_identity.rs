//! Blank lobby identity: omit the host's profile publication during creation.
//! This changes lobby display, not the authenticated Battle.net identity.
use serde_json::Value;
use std::time::{Duration, Instant};

#[cfg(windows)]
mod native;
#[cfg(windows)]
mod stub;
#[cfg(windows)]
use native::NativeFilter;
#[cfg(not(windows))]
pub(crate) struct NativeFilter;

pub(crate) trait SendFilter {
    fn install(&mut self) -> Result<(), String>;
    fn count(&self) -> Result<u64, String>;
    fn restore(&mut self) -> Result<(), String>;
}

pub(crate) struct Identity<F: SendFilter> {
    filter: F,
    creating: Option<Instant>,
}

pub(crate) type HostIdentity = Identity<NativeFilter>;

impl HostIdentity {
    pub fn open(pid: u32) -> Result<Self, String> {
        Ok(Self {
            filter: NativeFilter::open(pid)?,
            creating: None,
        })
    }
}

impl<F: SendFilter> Identity<F> {
    /// Call immediately before the native CreateLobby command, after reservation.
    pub fn begin_creation(&mut self) -> Result<(), String> {
        self.cancel_creation()?;
        self.filter.install()?;
        self.creating = Some(Instant::now());
        Ok(())
    }

    pub fn matches(value: &Value) -> bool {
        let Some(players) = value["players"].as_array() else {
            return false;
        };
        let mut own = players.iter().filter(|p| p["isSelf"] == true);
        value["isHost"] == true
            // The native serializer omits playerHost when its cached name is empty.
            && value.get("playerHost").is_none_or(|name| name.as_str() == Some(""))
            && own.next().is_some_and(|p| p["name"].as_str() == Some(""))
            && own.next().is_none()
    }

    /// Only accept a complete, freshly serialized lobby owned by this worker.
    pub fn confirm(&mut self, value: &Value) -> Result<bool, String> {
        if !Self::matches(value) {
            self.cancel_creation()?;
            return Err("The host name is not blank. Close this lobby, then click Host to create a new one.".into());
        }
        if self.creating.is_some() {
            match self.filter.count()? {
                0 => return Ok(false), // An early blank placeholder is not proof.
                1 => self.cancel_creation()?,
                _ => {
                    self.cancel_creation()?;
                    return Err("Unexpected host profile publication. Hosting stopped.".into());
                }
            }
        }
        Ok(true)
    }

    pub fn check_timeout(&mut self, budget: Duration) -> Result<(), String> {
        if self.creating.is_some_and(|since| since.elapsed() >= budget) {
            self.cancel_creation()?;
            return Err("The game did not confirm a blank host name.".into());
        }
        Ok(())
    }

    pub fn cancel_creation(&mut self) -> Result<(), String> {
        self.filter.restore()?;
        self.creating = None;
        Ok(())
    }
}

impl<F: SendFilter> Drop for Identity<F> {
    fn drop(&mut self) {
        let _ = self.filter.restore();
    }
}

#[cfg(not(windows))]
impl NativeFilter {
    fn open(_: u32) -> Result<Self, String> {
        Err("Blank host names require 64-bit Windows.".into())
    }
}
#[cfg(not(windows))]
impl SendFilter for NativeFilter {
    fn install(&mut self) -> Result<(), String> {
        Err("Unsupported platform.".into())
    }
    fn count(&self) -> Result<u64, String> {
        Err("Unsupported platform.".into())
    }
    fn restore(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
