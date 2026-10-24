use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Connecting,
    Observing,
    Backoff,
    Saving,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub age_seconds: u64,
    pub stall_after_seconds: u64,
    pub stage: Stage,
}

/// Only the hosting thread renews this lease. A responsive frontend is not proof
/// that the native client, transport, or hosting thread is making progress.
#[derive(Clone)]
pub(super) struct Progress(Arc<Mutex<(Instant, Stage)>>, u64);

impl Default for Progress {
    fn default() -> Self {
        Self::new(crate::config::default_startup_timeout())
    }
}

impl Progress {
    pub fn new(startup: u64) -> Self {
        Self(
            Arc::new(Mutex::new((Instant::now(), Stage::Connecting))),
            startup.clamp(60, 900) + 30,
        )
    }
    pub fn pulse(&self, stage: Stage) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = (Instant::now(), stage);
    }

    pub fn snapshot(&self) -> Health {
        let (seen, stage) = *self.0.lock().unwrap_or_else(|e| e.into_inner());
        Health {
            age_seconds: seen.elapsed().as_secs(),
            stage,
            stall_after_seconds: match stage {
                Stage::Connecting => self.1,
                Stage::Saving => 330,
                _ => 90,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_health_cannot_renew_a_stalled_worker() {
        let progress = Progress::default();
        progress.0.lock().unwrap().0 -= std::time::Duration::from_secs(100);
        assert!(progress.snapshot().age_seconds >= 100);
        assert!(progress.snapshot().age_seconds >= 100);
        progress.pulse(Stage::Backoff);
        assert_eq!(progress.snapshot().age_seconds, 0);
        assert_eq!(progress.snapshot().stage, Stage::Backoff);
    }
}
