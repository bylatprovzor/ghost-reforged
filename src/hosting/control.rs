use super::*;

pub(super) fn chat_command(
    engine: &Engine,
    chat: &mut crate::commands::LobbyCommands,
    event: Option<&Value>,
    identity_ok: bool,
    now: u64,
) -> crate::commands::Outcome {
    use crate::commands::ChatCommand;
    if !matches!(
        engine.phase,
        Phase::Lobby | Phase::Countdown(_) | Phase::Starting(_)
    ) {
        chat.observe(None, now);
        return Default::default();
    }
    chat.observe(engine.owned_lobby(now), now);
    let Some(event) = event.filter(|_| identity_ok) else {
        return Default::default();
    };
    let mut settings = engine.config.commands.clone();
    if matches!(engine.phase, Phase::Starting(_)) {
        settings
            .0
            .retain(|c| matches!(c, ChatCommand::Help | ChatCommand::Players));
    }
    chat.handle(event, &settings, now)
}

pub(super) fn dispatch(
    engine: &mut Engine,
    event: Option<&Value>,
    control: Option<&Control>,
    alias_ready: bool,
    now: u64,
) -> Vec<Command> {
    if let Some(Control::StartRule { minimum, full }) = control
        && (1..=23).contains(minimum)
    {
        engine.update_start_rule(*minimum, *full);
    }
    if let Some(Control::Settings(settings)) = control {
        let _ = settings.apply(engine, now);
    }
    let desired_auto = engine.config.auto_start;
    engine.config.auto_start = desired_auto && alias_ready;
    let mut commands = match event {
        Some(event) => engine.on_event(event, now),
        None => engine.tick(now),
    };
    engine.config.auto_start = desired_auto;
    commands.extend(match control {
        Some(Control::Start) if alias_ready => engine.start_now(now),
        Some(Control::Slot { slot, open }) => {
            engine.slot_command(*slot, *open, now).into_iter().collect()
        }
        _ => Vec::new(),
    });
    commands
}

#[derive(Debug, PartialEq)]
pub(super) enum Reservation {
    Saved,
    Cancelled,
}

pub(super) fn reserve(
    number: u32,
    rx: &Receiver<Control>,
    tx: &Sender<Event>,
    stop: &AtomicBool,
    deferred: &mut VecDeque<Control>,
) -> Result<Reservation, String> {
    if stop.load(Ordering::SeqCst) {
        return Ok(Reservation::Cancelled);
    }
    tx.send(Event::Reserve(number))
        .map_err(|_| "The launcher closed.")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return Ok(Reservation::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Control::Reserved {
                number: confirmed,
                saved,
            }) if number == confirmed => {
                return if saved {
                    Ok(Reservation::Saved)
                } else {
                    Err("The next lobby number could not be saved.".into())
                };
            }
            Ok(Control::Stop) => return Ok(Reservation::Cancelled),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("The launcher closed.".into()),
            Ok(control) => deferred.push_back(control),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    Err("Lobby number reservation was not acknowledged.".into())
}
