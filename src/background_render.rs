//! Reversible browser suspension. A separate process owns all suspend counts so
//! controller crashes and stalled hosting loops cannot leave the browser frozen.
mod process;
mod watchdog;

use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

pub(crate) const ARGUMENT: &str = "--render-guardian";
pub(crate) const LEASE: Duration = Duration::from_secs(4);

pub(crate) struct Guard {
    child: Child,
    input: Option<ChildStdin>,
    replies: Receiver<u8>,
    paused: bool,
    renewed: Instant,
}

impl Guard {
    pub fn start(pid: u32) -> Result<Self, String> {
        let game = process::Process::open(pid, false)?;
        let owner = process::Process::open(std::process::id(), false)?;
        let mut command = Command::new(
            std::env::current_exe().map_err(|_| "Cannot find the render guardian executable.")?,
        );
        #[cfg(test)]
        command
            .args([
                "--exact",
                "background_render::tests::guardian_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(
                "GHOST_RENDER_GUARDIAN_FIXTURE",
                format!("{} {} {} {}", owner.pid, owner.created, pid, game.created),
            );
        #[cfg(not(test))]
        command.arg(ARGUMENT).args([
            owner.pid.to_string(),
            owner.created.to_string(),
            pid.to_string(),
            game.created.to_string(),
        ]);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|_| "Cannot start the render guardian.")?;
        let input = child.stdin.take();
        let mut output = child.stdout.take().ok_or("Missing guardian output")?;
        let (tx, replies) = mpsc::channel();
        thread::Builder::new()
            .name("ghost-render-replies".into())
            .spawn(move || {
                let mut byte = [0];
                while output.read_exact(&mut byte).is_ok() {
                    if matches!(byte[0], b'P' | b'R' | b'E') && tx.send(byte[0]).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| "Cannot watch the render guardian.")?;
        let mut guard = Self {
            child,
            input,
            replies,
            paused: false,
            renewed: Instant::now(),
        };
        guard.request(false)?;
        Ok(guard)
    }

    fn request(&mut self, pause: bool) -> Result<(), String> {
        let byte = if pause { b'P' } else { b'R' };
        self.input
            .as_mut()
            .ok_or("Render guardian is closed")?
            .write_all(&[byte])
            .map_err(|_| "Render guardian is unavailable")?;
        match self.replies.recv_timeout(Duration::from_secs(2)) {
            Ok(reply) if reply == byte => {
                self.paused = pause;
                self.renewed = Instant::now();
                Ok(())
            }
            _ => {
                // EOF makes the guardian resume even if a response was lost.
                self.input.take();
                Err(
                    "Low-resource rendering could not be confirmed; browser suspension released."
                        .into(),
                )
            }
        }
    }

    pub fn set_paused(&mut self, pause: bool) -> Result<(), String> {
        if self.input.is_none() {
            return Err("Render guardian is closed.".into());
        }
        if self.paused != pause || (pause && self.renewed.elapsed() >= Duration::from_secs(1)) {
            self.request(pause)?;
        }
        Ok(())
    }
}

use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

impl Drop for Guard {
    fn drop(&mut self) {
        // Never kill the guardian: it owns the matching resume operations.
        self.input.take();
        let _ = self.child.try_wait();
    }
}

pub(crate) fn entry() -> bool {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_none_or(|a| a != ARGUMENT) {
        return false;
    }
    let _ = watchdog::run(&args[2..]);
    true
}

#[cfg(test)]
mod tests;
