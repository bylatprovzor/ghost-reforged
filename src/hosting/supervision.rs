use super::*;
use crate::discovery;

pub(super) fn attempt<T>(action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_or_else(|_| {
        Err("The hosting action failed unexpectedly; rechecking the native session.".into())
    })
}

/// These guards belong to the hosting intent, not to a socket attempt.
#[derive(Default)]
pub(super) struct Resources {
    pub progress: health::Progress,
    pub game: Option<background::Background>,
    pub identity: Option<HostIdentity>,
    pub launcher: background::LauncherLaunch,
    pub generation: u64,
    pub startup: crate::background_startup::Startup,
    pub initialization: connection::Initialization,
    pub login_recovery: super::login_recovery::Recovery,
}

impl Resources {
    pub fn clear_dead(&mut self) -> bool {
        if self.game.as_ref().is_some_and(|game| !game.running()) {
            self.identity = None;
            self.game = None;
            self.startup.ready();
            self.initialization = Default::default();
            self.login_recovery = Default::default();
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
    healthy_since: Option<Instant>,
}

impl Retry {
    pub fn observe(&mut self, engine: &Engine, identity_ready: bool, now: u64, at: Instant) {
        if !identity_ready
            || !matches!(engine.phase, Phase::Lobby | Phase::Countdown(_))
            || engine.owned_lobby(now).is_none()
        {
            self.healthy_since = None;
            return;
        }
        let since = self.healthy_since.get_or_insert(at);
        if at.saturating_duration_since(*since) >= Duration::from_secs(60) {
            self.failures = 0;
        }
    }

    pub fn delay(&mut self) -> Duration {
        self.healthy_since = None;
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
        resources.progress.pulse(health::Stage::Backoff);
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
        resources.progress.pulse(health::Stage::Connecting);
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
                if resources
                    .startup
                    .expired(Duration::from_secs(config.startup_timeout_seconds))
                    && let Some(game) = &mut resources.game
                    && resources.startup.owns(game.pid())
                    && !stop.load(Ordering::SeqCst)
                {
                    // Recheck immediately before resetting. A late listener revokes
                    // startup ownership even if its handshake is not yet ready.
                    match discovery::has_listener(game.pid()) {
                        Ok(false) => {
                            status(
                                tx,
                                "Native startup timed out. Restarting the requested client…",
                            );
                            game.terminate()?;
                            if resources.clear_dead() {
                                engine.client_lost(started.elapsed().as_secs());
                                *repair = Default::default();
                                let _ = tx.send(Event::ClientChanged(None));
                            }
                        }
                        Ok(true) => resources
                            .login_recovery
                            .listener_ready(&mut resources.startup, game.pid()),
                        Err(_) => {}
                    }
                }
                let delay = retry.delay();
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
            assert_eq!(retry.delay().as_secs(), seconds);
        }
        let mut engine = super::super::tests::ready(false);
        let began = Instant::now();
        for now in 0..=60 {
            engine.on_event(&super::super::tests::lobby(false), now);
            retry.observe(&engine, true, now, began + Duration::from_secs(now));
        }
        assert_eq!(retry.delay().as_secs(), 2);
    }

    #[test]
    fn an_open_socket_without_a_fresh_owned_lobby_never_resets_backoff() {
        let mut retry = Retry::default();
        let began = Instant::now();
        let mut engine = super::super::tests::ready(false);
        for (index, phase) in [
            Phase::AwaitSignIn,
            Phase::Creating(0),
            Phase::Recovering(0),
            Phase::Starting(0),
        ]
        .into_iter()
        .enumerate()
        {
            engine.phase = phase;
            retry.observe(
                &engine,
                true,
                0,
                began + Duration::from_secs(index as u64 * 1000),
            );
            retry.observe(
                &engine,
                true,
                0,
                began + Duration::from_secs(index as u64 * 1000 + 70),
            );
            assert_eq!(retry.delay().as_secs(), [2, 5, 10, 30][index]);
        }
        engine.phase = Phase::Lobby;
        let began = began + Duration::from_secs(4000);
        retry.observe(&engine, true, 0, began);
        retry.observe(&engine, false, 0, began + Duration::from_secs(59));
        retry.observe(&engine, true, 0, began + Duration::from_secs(60));
        assert_eq!(retry.delay().as_secs(), 60);
        let began = began + Duration::from_secs(1000);
        retry.observe(&engine, true, 0, began);
        retry.observe(&engine, true, 1000, began + Duration::from_secs(60));
        assert_eq!(retry.delay().as_secs(), 60);
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
