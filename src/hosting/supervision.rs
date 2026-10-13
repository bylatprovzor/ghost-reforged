use super::*;

pub(super) fn attempt<T>(action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_or_else(|_| {
        Err("The hosting action failed unexpectedly; rechecking the native session.".into())
    })
}

/// These guards belong to the hosting intent, not to a socket attempt.
#[derive(Default)]
pub(super) struct Resources {
    pub game: Option<background::Background>,
    pub identity: Option<HostIdentity>,
    pub launcher: background::LauncherLaunch,
    pub generation: u64,
}

impl Resources {
    pub fn clear_dead(&mut self) -> bool {
        if self.game.as_ref().is_some_and(|game| !game.running()) {
            self.identity = None;
            self.game = None;
            self.generation = self.generation.wrapping_add(1);
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
pub(super) struct Retry {
    failures: u32,
}

impl Retry {
    pub fn delay(&mut self, stable: Duration) -> Duration {
        if stable >= Duration::from_secs(60) {
            self.failures = 0;
        }
        let seconds = [2, 5, 10, 30, 60][self.failures.min(4) as usize];
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(seconds)
    }
}

pub(super) fn wait(
    delay: Duration,
    rx: &Receiver<Control>,
    stop: &AtomicBool,
    engine: &mut Engine,
    now: u64,
    resources: &mut Resources,
) -> bool {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        // Ignore old slot/start actions: they referred to the failed session.
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Ok(Control::Settings(settings)) => {
                let _ = settings.apply(engine, now);
            }
            Ok(Control::StartRule { minimum, full }) if (1..=23).contains(&minimum) => {
                engine.update_start_rule(minimum, full);
            }
            _ => {}
        }
        let _ = resources.launcher.poll(stop, deadline);
    }
    !stop.load(Ordering::SeqCst)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn connect(
    engine: &mut Engine,
    resources: &mut Resources,
    repair: &mut repair::Repair,
    reserved: Option<u32>,
    rx: &Receiver<Control>,
    tx: &Sender<Event>,
    stop: &AtomicBool,
    started: Instant,
    retry: &mut Retry,
) -> Result<Option<connection::Connection>, String> {
    while !stop.load(Ordering::SeqCst) {
        if resources.clear_dead() {
            engine.client_lost(started.elapsed().as_secs());
            *repair = Default::default();
            let _ = tx.send(Event::ClientChanged(None));
        }
        if matches!(&engine.phase, Phase::Stopped(reason) if reason == "Configured game limit reached")
        {
            return Ok(None);
        }
        let mut config = engine.config.clone();
        if reserved == Some(engine.number()) || engine.shutdown_identity().is_some() {
            config.resume_lobby = true;
            config.first_number = engine
                .number()
                .checked_add(1)
                .ok_or("Lobby number exhausted")?;
        }
        let restored = resources
            .identity
            .as_mut()
            .map_or(Ok(()), |identity| identity.cancel_creation());
        if let Some(game) = resources.game.as_mut() {
            game.low_resource(false)?;
        }
        let generation = resources.generation;
        let opened = attempt(|| {
            restored.and_then(|()| connection::open(&config, tx, stop, repair, resources))
        });
        // Also cover a process exiting inside discovery/connection, after the
        // check at the top of the loop. Old engine ownership cannot cross it.
        if resources.generation != generation {
            engine.client_lost(started.elapsed().as_secs());
            *repair = Default::default();
        }
        match opened {
            Ok(Some(connection)) => return Ok(Some(connection)),
            Ok(None) => return Ok(None),
            Err(error) => {
                let delay = retry.delay(Duration::ZERO);
                status(
                    tx,
                    &format!("{error} Retrying automatically in {} s…", delay.as_secs()),
                );
                if !wait(
                    delay,
                    rx,
                    stop,
                    engine,
                    started.elapsed().as_secs(),
                    resources,
                ) {
                    return Ok(None);
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_panic_enters_recovery_instead_of_silently_losing_the_worker() {
        let failed: Result<(), String> = attempt(|| panic!("simulated worker action"));
        assert!(failed.is_err());
        assert_eq!(attempt(|| Ok(42)).unwrap(), 42);
    }

    #[test]
    fn retries_never_exhaust_and_reset_after_a_healthy_minute() {
        let mut retry = Retry::default();
        for seconds in [2, 5, 10, 30, 60, 60, 60] {
            assert_eq!(retry.delay(Duration::ZERO).as_secs(), seconds);
        }
        assert_eq!(retry.delay(Duration::from_secs(60)).as_secs(), 2);
    }

    #[test]
    fn stop_interrupts_backoff_and_queued_actions_are_not_replayed() {
        let (tx, rx) = mpsc::channel();
        tx.send(Control::Start).unwrap();
        tx.send(Control::StartRule {
            minimum: 4,
            full: true,
        })
        .unwrap();
        tx.send(Control::Stop).unwrap();
        let mut engine = Engine::new(Config::default());
        let began = Instant::now();
        assert!(!wait(
            Duration::from_secs(60),
            &rx,
            &AtomicBool::new(false),
            &mut engine,
            0,
            &mut Resources::default()
        ));
        assert_eq!(engine.config.min_players, 4);
        assert!(began.elapsed() < Duration::from_secs(1));
        assert!(rx.try_recv().is_err());
    }
}
