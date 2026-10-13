use super::{
    LEASE,
    process::{Family, Process, parent_pid, same_path},
};
use std::{
    io::{Read, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) fn run(args: &[String]) -> Result<(), String> {
    let [owner, owner_created, game, game_created] = args else {
        return Err("Invalid render guardian arguments".into());
    };
    let owner: u32 = owner.parse().map_err(|_| "Invalid owner")?;
    let owner_created: u64 = owner_created
        .parse()
        .map_err(|_| "Invalid owner lifetime")?;
    if parent_pid(std::process::id())? != owner {
        return Err("Render guardian must be started by its owner".into());
    }
    let owner = Process::open(owner, false)?;
    if owner.created != owner_created
        || !same_path(
            &owner.path,
            &std::env::current_exe().map_err(|_| "Missing executable")?,
        )
    {
        return Err("Render guardian owner changed".into());
    }
    let mut family = Family::new(
        game.parse().map_err(|_| "Invalid game")?,
        game_created.parse().map_err(|_| "Invalid game lifetime")?,
    )?;
    let (tx, rx) = mpsc::sync_channel(8);
    thread::Builder::new()
        .name("ghost-render-input".into())
        .spawn(move || {
            let mut input = std::io::stdin().lock();
            let mut byte = [0];
            while input.read_exact(&mut byte).is_ok() {
                if tx.send(byte[0]).is_err() {
                    break;
                }
            }
        })
        .map_err(|_| "Cannot monitor render guardian input")?;
    let mut renewed = Instant::now();
    let mut output = std::io::stdout().lock();
    loop {
        if !owner.running() || !family.running() {
            return family.resume();
        }
        if renewed.elapsed() >= LEASE {
            family.resume()?;
        }
        let message = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return family.resume(),
        };
        let result = match message {
            b'P' => family.pause(),
            b'R' => family.resume(),
            _ => Err("Invalid render guardian command".into()),
        };
        if result.is_err() {
            let _ = family.resume();
            let _ = output.write_all(b"E");
            let _ = output.flush();
            return result;
        }
        renewed = Instant::now();
        output
            .write_all(&[message])
            .map_err(|_| "Render guardian owner disconnected")?;
        output
            .flush()
            .map_err(|_| "Render guardian owner disconnected")?;
    }
}
