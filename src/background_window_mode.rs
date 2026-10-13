//! A requested window mode is not ready until the native worker observes it.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct Mode {
    requested: AtomicU64,
    completed: AtomicU64,
}

impl Mode {
    pub fn request(&self, enabled: bool) -> u64 {
        let old = self
            .requested
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                ((old & 1 != 0) != enabled).then_some(((old + 2) & !1) | u64::from(enabled))
            });
        match old {
            Ok(old) => ((old + 2) & !1) | u64::from(enabled),
            Err(current) => current,
        }
    }

    pub fn ticket(&self) -> u64 {
        self.requested.load(Ordering::SeqCst)
    }

    pub fn enabled(&self) -> bool {
        self.ticket() & 1 != 0
    }

    pub fn acknowledge(&self, ticket: u64) {
        if self.ticket() == ticket {
            self.completed.store(ticket, Ordering::SeqCst);
        }
    }

    pub fn ready(&self, ticket: u64) -> bool {
        self.completed.load(Ordering::SeqCst) == ticket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_minimize_completion_cannot_acknowledge_a_later_restore() {
        let mode = Mode::default();
        let minimize = mode.request(true);
        let restore = mode.request(false);
        mode.acknowledge(minimize);
        assert!(!mode.ready(restore));
        mode.acknowledge(restore);
        assert!(mode.ready(restore));
        assert!(!mode.enabled());
    }
}
