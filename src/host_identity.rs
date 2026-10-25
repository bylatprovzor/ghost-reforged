//! Blank lobby identity: omit the host's profile publication during creation.
//! This changes lobby display, not the authenticated Battle.net identity.
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// Named-host fallback: on Warcraft III builds where the blank-name filter has not been
// verified (for example 3.0.1.24371), host with the account's own name in the observer slot.
// The ladder only reads the four human slots, so the visible host name does not matter.
static NAMED: AtomicBool = AtomicBool::new(false);

pub fn named_host() -> bool {
    NAMED.load(Ordering::Relaxed)
}

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
    filter: Option<F>,
    creating: Option<Instant>,
}

pub(crate) type HostIdentity = Identity<NativeFilter>;

impl HostIdentity {
    pub fn open(pid: u32) -> Result<Self, String> {
        match NativeFilter::open(pid) {
            Ok(filter) => {
                NAMED.store(false, Ordering::Relaxed);
                Ok(Self {
                    filter: Some(filter),
                    creating: None,
                })
            }
            Err(error) if std::env::var_os("GHOST_REQUIRE_BLANK_HOST").is_some() => Err(error),
            Err(_) => {
                NAMED.store(true, Ordering::Relaxed);
                Ok(Self {
                    filter: None,
                    creating: None,
                })
            }
        }
    }
}

impl<F: SendFilter> Identity<F> {
    /// Call immediately before the native CreateLobby command, after reservation.
    pub fn begin_creation(&mut self) -> Result<(), String> {
        self.cancel_creation()?;
        if let Some(filter) = &mut self.filter {
            filter.install()?;
        }
        self.creating = Some(Instant::now());
        Ok(())
    }

    pub fn matches(value: &Value) -> bool {
        let Some(players) = value["players"].as_array() else {
            return false;
        };
        let mut own = players.iter().filter(|p| p["isSelf"] == true);
        if named_host() {
            return value["isHost"] == true && own.next().is_some() && own.next().is_none();
        }
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
            let count = match &self.filter {
                Some(filter) => Some(filter.count()?),
                None => None,
            };
            match count {
                None => self.cancel_creation()?, // named host: nothing to prove
                Some(0) => return Ok(false), // An early blank placeholder is not proof.
                Some(1) => self.cancel_creation()?,
                Some(_) => {
                    self.cancel_creation()?;
                    return Err("Unexpected host profile publication. Hosting stopped.".into());
                }
            }
        }
        Ok(true)
    }

    pub fn check_timeout(&mut self) -> Result<(), String> {
        if self
            .creating
            .is_some_and(|since| since.elapsed() > Duration::from_secs(30))
        {
            self.cancel_creation()?;
            return Err("The game did not confirm a blank host name.".into());
        }
        Ok(())
    }

    pub fn cancel_creation(&mut self) -> Result<(), String> {
        if let Some(filter) = &mut self.filter {
            filter.restore()?;
        }
        self.creating = None;
        Ok(())
    }
}

impl<F: SendFilter> Drop for Identity<F> {
    fn drop(&mut self) {
        if let Some(filter) = &mut self.filter {
            let _ = filter.restore();
        }
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
