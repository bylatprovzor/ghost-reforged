use crate::console::Health;
use std::time::{Duration, Instant};

pub const START_GRACE: Duration = Duration::from_secs(120);
pub const STALL_LIMIT: u64 = 90;

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Wait,
    Stop,
    Restart,
    Foreign,
}

pub fn inspect(pid: u32, health: &Health) -> Decision {
    if health.version != 1 || health.pid != pid {
        return Decision::Foreign;
    }
    if health.quitting {
        return Decision::Stop;
    }
    match &health.worker {
        Some(worker) if !(90..=930).contains(&worker.stall_after_seconds) => Decision::Foreign,
        Some(worker) if worker.age_seconds >= worker.stall_after_seconds => Decision::Restart,
        None if health.hosting_requested => Decision::Restart,
        _ => Decision::Wait,
    }
}

#[derive(Default)]
pub struct Backoff {
    failures: u32,
    responsive: Option<(Instant, Instant)>,
}
impl Backoff {
    pub fn observe(&mut self, pid: u32, health: Option<&Health>, now: Instant) {
        // Lease validity permits slow work; it does not prove recent progress.
        // Require consecutive fresh replies before forgetting earlier failures.
        let fresh = health.is_some_and(|health| {
            inspect(pid, health) == Decision::Wait
                && health
                    .worker
                    .as_ref()
                    .is_none_or(|worker| worker.age_seconds <= 15)
        });
        if !fresh {
            self.responsive = None;
            return;
        }
        let since = match self.responsive {
            Some((since, last))
                if now.saturating_duration_since(last) <= Duration::from_secs(15) =>
            {
                since
            }
            _ => now,
        };
        self.responsive = Some((since, now));
        if now.saturating_duration_since(since) >= Duration::from_secs(300) {
            self.failures = 0;
        }
    }

    pub fn next(&mut self) -> Duration {
        self.responsive = None;
        let seconds = [2, 5, 15, 60, 180, 300][self.failures.min(5) as usize];
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn health() -> Health {
        Health {
            version: 1,
            pid: 42,
            worker: None,
            hosting_requested: false,
            quitting: false,
        }
    }
    #[test]
    fn only_a_verified_owned_worker_can_trigger_recovery() {
        let mut value = health();
        assert_eq!(inspect(42, &value), Decision::Wait);
        value.hosting_requested = true;
        assert_eq!(inspect(42, &value), Decision::Restart);
        value.quitting = true;
        assert_eq!(inspect(42, &value), Decision::Stop);
        assert_eq!(inspect(43, &value), Decision::Foreign);
    }
    #[test]
    fn slow_startup_is_healthy_while_the_worker_is_progressing() {
        let mut value = health();
        value.hosting_requested = true;
        value.worker = Some(
            serde_json::from_value(serde_json::json!({
                "age_seconds":329,"stage":"connecting","stall_after_seconds":330
            }))
            .unwrap(),
        );
        assert_eq!(inspect(42, &value), Decision::Wait);
        value.worker.as_mut().unwrap().age_seconds = 330;
        assert_eq!(inspect(42, &value), Decision::Restart);
    }
    #[test]
    fn repeated_failure_backs_off_but_a_stable_run_resets_the_budget() {
        let mut policy = Backoff::default();
        for expected in [2, 5, 15, 60, 180, 300, 300] {
            assert_eq!(policy.next().as_secs(), expected);
        }
        let start = Instant::now();
        for second in (0..=300).step_by(5) {
            policy.observe(42, Some(&health()), start + Duration::from_secs(second));
        }
        assert_eq!(policy.next().as_secs(), 2);
    }

    #[test]
    fn surviving_a_startup_timeout_is_not_evidence_of_recovery() {
        let mut policy = Backoff::default();
        let mut value = health();
        value.hosting_requested = true;
        value.worker = Some(
            serde_json::from_value(serde_json::json!({
                "age_seconds":0,"stage":"connecting","stall_after_seconds":930
            }))
            .unwrap(),
        );
        let mut now = Instant::now();
        // A blocked startup can remain alive for the full 930-second lease.
        for expected in [2, 5, 15] {
            for second in (0..=930).step_by(5) {
                value.worker.as_mut().unwrap().age_seconds = second;
                policy.observe(42, Some(&value), now + Duration::from_secs(second));
            }
            assert_eq!(policy.next().as_secs(), expected);
            now += Duration::from_secs(940);
        }
    }

    #[test]
    fn missing_or_sparse_replies_do_not_reset_recovery_backoff() {
        let mut policy = Backoff::default();
        assert_eq!(policy.next().as_secs(), 2);
        let start = Instant::now();
        for second in (0..300).step_by(5) {
            policy.observe(42, Some(&health()), start + Duration::from_secs(second));
        }
        policy.observe(42, None, start + Duration::from_secs(300));
        for second in [305, 605, 905] {
            policy.observe(42, Some(&health()), start + Duration::from_secs(second));
        }
        assert_eq!(policy.next().as_secs(), 5);
    }

    #[test]
    fn wrong_controller_and_quitting_do_not_count_as_recovery() {
        let mut policy = Backoff::default();
        assert_eq!(policy.next().as_secs(), 2);
        let start = Instant::now();
        for second in (0..=300).step_by(5) {
            policy.observe(43, Some(&health()), start + Duration::from_secs(second));
        }
        assert_eq!(policy.next().as_secs(), 5);
        let mut value = health();
        value.quitting = true;
        for second in (305..=610).step_by(5) {
            policy.observe(42, Some(&value), start + Duration::from_secs(second));
        }
        assert_eq!(policy.next().as_secs(), 15);
    }
}
