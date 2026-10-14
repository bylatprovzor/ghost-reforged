use super::*;

enum Step {
    Continue,
    Reconnect,
    Stop,
    Shutdown(Result<(), String>),
}

pub(super) fn run(
    config: Config,
    rx: &Receiver<Control>,
    tx: &Sender<Event>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let _lease = background::HostLease::acquire()?;
    if stop.load(Ordering::SeqCst) {
        return Ok(());
    }
    let mut repair = repair::Repair::default();
    let mut resources = supervision::Resources::default();
    let mut retry = supervision::Retry::default();
    let mut engine = Engine::new(config);
    let started = Instant::now();
    let mut reserved = None;
    let Some(connection) = supervision::connect(
        &mut engine,
        &mut resources,
        &mut repair,
        reserved,
        rx,
        tx,
        stop,
        started,
        &mut retry,
    )?
    else {
        return Ok(());
    };
    if connection.initialization_required {
        engine.expect_client_initialization();
    }
    let mut client = connection.client;
    let mut pending = connection.pending;
    let mut connected = Instant::now();
    let mut requested = Instant::now();
    let mut reported = String::new();
    let mut snapshot = None;
    let mut deferred_controls = VecDeque::new();
    let mut identity_ok = false;
    let _ = tx.send(Event::ClientChanged(
        resources.game.as_ref().map(|game| game.pid()),
    ));
    let mut visual_state = None;
    let mut activity = activity::ActivityTracker::default();
    let mut chat = super::chat::Chat::new(engine.config.stats_path.as_deref());
    let mut announcements = crate::welcome::Announcements::default();
    let mut start_requested = None;
    let mut kick_requested: Option<(crate::commands::KickTarget, u64, kick::RefreshGate)> = None;
    let mut manual_kick = kick::PendingKick::default();
    let mut slot_edit = slots::PendingSlot::default();
    let mut ladder_seed = ladder::Seed::default();
    loop {
        let step = supervision::attempt(|| -> Result<Step, String> {
            let game = resources
                .game
                .as_mut()
                .ok_or("Warcraft III is unavailable")?;
            let identity = resources
                .identity
                .as_mut()
                .ok_or("Host identity is unavailable")?;
            resources
                .launcher
                .watch(stop, Instant::now() + Duration::from_secs(2))?;
            identity.check_timeout()?;
            let now = started.elapsed().as_secs();
            let control = deferred_controls.pop_front().or_else(|| rx.try_recv().ok());
            if stop.load(Ordering::SeqCst) || matches!(control, Some(Control::Stop)) {
                let resumed = game.low_resource(false);
                let restored = identity.cancel_creation();
                return Ok(Step::Shutdown(shutdown::finish(resumed, restored, || {
                    shutdown::close(&engine, &mut client, &mut pending)
                })));
            }
            if let Some(Control::Settings(settings)) = &control {
                chat.apply_settings(&settings.commands);
                if !settings
                    .commands
                    .enabled(crate::commands::ChatCommand::VoteKick)
                {
                    kick_requested = None;
                }
            }
            if matches!(control, Some(Control::Start)) {
                start_requested = Some(now);
            }
            let event = if let Some(event) = pending.pop_front() {
                Some(event)
            } else {
                client
                    .receive_with_timeout(Duration::from_millis(50))
                    .map_err(|e| e.to_string())?
            };
            if event.as_ref().is_some_and(|e| {
                e["messageType"] == "GameLobbySetup" && !HostIdentity::matches(&e["payload"])
            }) {
                identity_ok = false;
            }
            let repair_lobby =
                if let Some(candidate) = repair.observe(&engine.phase, event.as_ref(), now) {
                    repair.confirm(&candidate, &mut client, &mut pending, tx, stop)?
                } else {
                    false
                };
            let mut commands = if repair_lobby {
                Vec::new()
            } else {
                if let Some(event) = &event
                    && event["messageType"] == "GameLobbySetup"
                {
                    ladder_seed.observe();
                    let ready = serde_json::from_value::<Lobby>(event["payload"].clone())
                        .is_ok_and(|lobby| ladder_seed.ready(&lobby));
                    engine.set_ladder_ready(ready);
                }
                dispatch(
                    &mut engine,
                    event.as_ref(),
                    control.as_ref(),
                    identity_ok,
                    now,
                )
            };
            if let Some(notice) = engine.take_notice() {
                let _ = tx.send(Event::Activity(Activity::Command(notice)));
            }
            if !matches!(
                engine.phase,
                Phase::Creating(_)
                    | Phase::Lobby
                    | Phase::Countdown(_)
                    | Phase::Starting(_)
                    | Phase::Recovering(_)
            ) {
                identity.cancel_creation()?;
                identity_ok = false;
            }
            if let Some(event) = &event
                && event["messageType"] == "GameLobbySetup"
                && engine.owned_lobby(now).is_some()
            {
                identity_ok = identity.confirm(&event["payload"])?;
            }
            commands.extend(ladder_seed.advance(&engine, now, event.is_none(), identity_ok));
            if let Some(lobby) = engine.owned_lobby(now) {
                engine.set_ladder_ready(ladder_seed.ready(lobby));
            }
            if let Some(notice) = ladder_seed.notice() {
                let _ = tx.send(Event::Activity(Activity::Command(notice)));
            }
            if let Some((command, line)) = manual_kick.confirm(
                &engine,
                now,
                event
                    .as_ref()
                    .is_some_and(|e| e["messageType"] == "GameLobbySetup"),
                identity_ok,
                event.is_none(),
            ) {
                commands.extend(command);
                if let Some(line) = line {
                    let _ = tx.send(Event::Activity(Activity::Command(line)));
                }
            }
            if let Some(Control::Kick(target)) = &control {
                manual_kick.request(target.clone(), &engine, now);
            }
            if let Some(Control::EditSlot(request)) = &control {
                slot_edit.request(request.clone(), &engine, now);
            }
            if identity_ok && event.is_none() {
                slot_edit.close_empty_observer(&engine, now);
            }
            let (slot_commands, line) = slot_edit.advance(
                &engine,
                now,
                event
                    .as_ref()
                    .is_some_and(|e| e["messageType"] == "GameLobbySetup"),
                event.is_none(),
                identity_ok,
            );
            commands.extend(slot_commands);
            if let Some(line) = line {
                let _ = tx.send(Event::Activity(Activity::Command(line)));
            }
            kick::invalidate_vote(&mut kick_requested, &engine, now);
            if let Some((_, _, gate)) = &mut kick_requested {
                match gate.advance(
                    event
                        .as_ref()
                        .is_some_and(|e| e["messageType"] == "GameLobbySetup"),
                    event.is_none(),
                    identity_ok,
                ) {
                    kick::Refresh::Request => commands.push(Command::empty("SendGameLobbySetup")),
                    kick::Refresh::Confirm => {
                        if let Some((target, _, _)) = kick_requested.take()
                            && let Some(lobby) = engine.owned_lobby(now)
                        {
                            commands.extend(target.confirm(lobby));
                        }
                    }
                    kick::Refresh::Wait => {}
                }
            }
            if let Some(requested) = start_requested {
                if now.saturating_sub(requested) >= 5 || matches!(engine.phase, Phase::Starting(_))
                {
                    start_requested = None;
                } else if identity_ok && engine.can_start(now) {
                    commands.extend(engine.start_now(now));
                    start_requested = None;
                }
            }
            let outcome = chat.process(
                &engine,
                event.as_ref(),
                identity_ok,
                started.elapsed(),
                &mut commands,
            );
            if let Some(warning) = chat.warning() {
                let _ = tx.send(Event::Activity(Activity::Command(warning)));
            }
            if let Some(line) = outcome.activity {
                let _ = tx.send(Event::Activity(Activity::Command(line)));
            }
            commands.extend(outcome.reply);
            if let Some(target) = outcome.kick {
                kick_requested = Some((target, now, kick::RefreshGate::default()));
            }
            if outcome.start {
                commands.extend(engine.start_now(now));
            }
            for event in activity.observe(
                &engine,
                now,
                event
                    .as_ref()
                    .is_some_and(|event| event["messageType"] == "GameLobbySetup"),
            ) {
                if let Activity::GameStarted(name) = &event {
                    chat.started(name);
                }
                track_announcement(&event, &mut announcements);
                let _ = tx.send(Event::Activity(event));
            }
            if let Some(event) = &event {
                match event["messageType"].as_str() {
                    Some("GameLobbySetup") if visual_state.as_ref() != Some(&event["payload"]) => {
                        visual_state = Some(event["payload"].clone());
                        let _ = tx.send(Event::VisualChanged);
                    }
                    Some("SetGlueScreen" | "LobbyCountdown") => {
                        let _ = tx.send(Event::VisualChanged);
                    }
                    _ => {}
                }
            }
            publish_lobby(&engine, identity_ok, now, &mut snapshot, tx);
            if repair_lobby || engine.phase == Phase::Restarting {
                if stop.load(Ordering::SeqCst) {
                    let resumed = game.low_resource(false);
                    let restored = identity.cancel_creation();
                    return Ok(Step::Shutdown(shutdown::finish(resumed, restored, || {
                        shutdown::close(&engine, &mut client, &mut pending)
                    })));
                }
                status(
                    tx,
                    if repair_lobby {
                        "Incomplete lobby detected. Restarting Warcraft III…"
                    } else {
                        "Restarting Warcraft III…"
                    },
                );
                let _ = tx.send(Event::ClientChanged(None));
                identity.cancel_creation()?;
                game.terminate()?;
                if repair_lobby {
                    engine.retry_incomplete_lobby();
                } else {
                    engine.client_stopped(now);
                }
                snapshot = None;
                identity_ok = false;
                let _ = tx.send(Event::LobbyCleared);
                if let Phase::Stopped(reason) = &engine.phase {
                    return if reason == "Configured game limit reached" {
                        Ok(Step::Stop)
                    } else {
                        Err(reason.clone())
                    };
                }
                return Ok(Step::Reconnect);
            }
            // Resume is acknowledged before LobbyStart (manual, chat or automatic),
            // creation, departure, or any loading/recovery transition can proceed.
            game.low_resource(render::pause_browser(&engine, identity_ok, now))?;
            for command in commands {
                if command.message == "CreateLobby" {
                    ladder_seed.reset();
                    engine.set_ladder_ready(false);
                    identity_ok = false;
                    let number = engine.number();
                    if reserved != Some(number) {
                        match reserve(number, rx, tx, stop, &mut deferred_controls) {
                            Ok(Reservation::Cancelled) => return Ok(Step::Stop),
                            Ok(Reservation::Saved) => {}
                            Err(error) => {
                                engine.retry_incomplete_lobby(); // CreateLobby was not sent.
                                return Err(error);
                            }
                        }
                        reserved = Some(number);
                    }
                }
                if stop.load(Ordering::SeqCst) {
                    // CreateLobby has not been sent yet in this iteration.
                    if command.message == "CreateLobby" {
                        return Ok(Step::Stop);
                    }
                    let resumed = game.low_resource(false);
                    let restored = identity.cancel_creation();
                    return Ok(Step::Shutdown(shutdown::finish(resumed, restored, || {
                        shutdown::close(&engine, &mut client, &mut pending)
                    })));
                }
                if command.message == "CreateLobby"
                    && let Err(error) = identity.begin_creation()
                {
                    engine.retry_incomplete_lobby(); // CreateLobby was not sent.
                    return Err(error);
                }
                client
                    .send(command.message, command.payload)
                    .map_err(|e| e.to_string())?;
            }
            publish_lobby(
                &engine,
                identity_ok,
                started.elapsed().as_secs(),
                &mut snapshot,
                tx,
            );
            if identity_ok
                && matches!(engine.phase, Phase::Lobby | Phase::Countdown(_))
                && let Some(lobby) = engine.owned_lobby(now)
                && let Some(command) =
                    announcements.next(lobby, &engine.config.welcome, now, &mut chat.lookup)
            {
                client
                    .send(command.message, command.payload)
                    .map_err(|e| e.to_string())?;
            }
            if !matches!(engine.phase, Phase::AwaitSignIn | Phase::Stopped(_)) {
                game.hide()?;
            }
            let phase = phase_label(&engine.phase, now);
            if phase != reported {
                status(tx, &phase);
                reported = phase;
                if engine.phase == Phase::AwaitSignIn {
                    return Err("Battle.net sign-in has not completed yet.".into());
                }
            }
            if let Phase::Stopped(reason) = &engine.phase {
                if reason == "Configured game limit reached" {
                    return Ok(Step::Stop);
                }
                return Err(reason.clone());
            }
            if (engine.awaiting_session() || matches!(engine.phase, Phase::Recovering(_)))
                && connected.elapsed() >= Duration::from_secs(60)
            {
                return Err("Waiting for a fresh Battle.net session.".into());
            }
            if requested.elapsed() >= snapshot_interval(&engine) {
                if engine.wants_snapshot() {
                    client
                        .send("SendGameLobbySetup", json!({}))
                        .map_err(|e| e.to_string())?;
                    if matches!(engine.phase, Phase::Recovering(_)) {
                        client
                            .send("GetGameList", json!({}))
                            .map_err(|e| e.to_string())?;
                    }
                } else if engine.awaiting_session() {
                    client
                        .send("GetGameList", json!({}))
                        .map_err(|e| e.to_string())?;
                }
                requested = Instant::now();
            }
            Ok(Step::Continue)
        });
        match step {
            Ok(Step::Stop) => return Ok(()),
            Ok(Step::Shutdown(result)) => return result,
            Ok(Step::Continue) => continue,
            Ok(Step::Reconnect) => {
                resources.clear_dead();
            }
            Err(error) => {
                if let Some(game) = resources.game.as_mut() {
                    let _ = game.low_resource(false);
                }
                if stop.load(Ordering::SeqCst) {
                    return shutdown::finish(Err(error), Ok(()), || {
                        shutdown::stop(&mut resources, &engine, &mut client, &mut pending)
                    });
                }
                if let Some(identity) = resources.identity.as_mut() {
                    let _ = identity.cancel_creation();
                }
                engine.recover(started.elapsed().as_secs());
                let delay = retry.delay(connected.elapsed());
                status(
                    tx,
                    &format!("{error} Retrying automatically in {} s…", delay.as_secs()),
                );
                if !supervision::wait(
                    delay,
                    rx,
                    stop,
                    &mut engine,
                    started.elapsed().as_secs(),
                    &mut resources,
                ) {
                    return shutdown::stop(&mut resources, &engine, &mut client, &mut pending);
                }
            }
        }
        identity_ok = false;
        snapshot = None;
        visual_state = None;
        start_requested = None;
        kick_requested = None;
        manual_kick = Default::default();
        slot_edit = Default::default();
        ladder_seed.reset();
        chat.reset();
        deferred_controls
            .retain(|control| matches!(control, Control::Settings(_) | Control::StartRule { .. }));
        let _ = tx.send(Event::LobbyCleared);
        let Some(connection) = supervision::connect(
            &mut engine,
            &mut resources,
            &mut repair,
            reserved,
            rx,
            tx,
            stop,
            started,
            &mut retry,
        )?
        else {
            return shutdown::stop(&mut resources, &engine, &mut client, &mut pending);
        };
        if connection.initialization_required {
            engine.expect_client_initialization();
        }
        client = connection.client;
        pending = connection.pending;
        connected = Instant::now();
        requested = Instant::now();
        reported.clear();
        let _ = tx.send(Event::ClientChanged(
            resources.game.as_ref().map(|game| game.pid()),
        ));
    }
}
