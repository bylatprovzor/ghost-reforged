//! Retry local render setup without discarding the authenticated native socket.
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

pub(super) fn prepare(
    stop: &AtomicBool,
    budget: Duration,
    mut pause: impl FnMut() -> Result<(), String>,
    mut notice: impl FnMut(&str),
) -> Result<bool, String> {
    let deadline = Instant::now() + budget;
    let mut reported = false;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(false);
        }
        match pause() {
            Ok(()) => return Ok(!stop.load(Ordering::SeqCst)),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(error);
                }
                if !reported {
                    notice(&error);
                    reported = true;
                }
            }
        }
        let retry = (Instant::now() + Duration::from_millis(250)).min(deadline);
        while Instant::now() < retry {
            if stop.load(Ordering::SeqCst) {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transient_render_failure_retries_locally_and_keeps_the_pending_create() {
        let mut attempts = 0;
        let mut notices = 0;
        let result = prepare(
            &AtomicBool::new(false),
            Duration::from_secs(2),
            || {
                attempts += 1;
                if attempts == 1 {
                    Err("guardian exited".into())
                } else {
                    Ok(())
                }
            },
            |_| notices += 1,
        );
        assert_eq!(result, Ok(true));
        assert_eq!(attempts, 2);
        assert_eq!(notices, 1);
    }

    #[test]
    fn cancellation_and_persistent_failure_never_authorize_creation() {
        let stop = AtomicBool::new(false);
        assert_eq!(
            prepare(
                &stop,
                Duration::from_secs(2),
                || {
                    stop.store(true, Ordering::SeqCst);
                    Err("failed".into())
                },
                |_| {}
            ),
            Ok(false)
        );
        assert!(
            prepare(
                &AtomicBool::new(false),
                Duration::ZERO,
                || Err("failed".into()),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(
            prepare(
                &AtomicBool::new(true),
                Duration::ZERO,
                || panic!("must not run"),
                |_| {}
            ),
            Ok(false)
        );
    }
}
