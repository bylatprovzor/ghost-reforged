//! Console frontend and same-user local control; never initializes Iced or captures images.
mod args;
#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod pipe_client;
#[cfg(windows)]
mod security;
mod session;
pub use args::{Args, Command};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Status,
    Host,
    Stop,
    Start,
    Quit,
    Profiles,
    ProfileGet {
        profile: Option<String>,
    },
    ProfileSelect {
        profile: String,
    },
    ProfileNew {
        title: String,
    },
    ProfileSet {
        profile: Option<String>,
        settings: Value,
    },
    Slot {
        lobby: String,
        number: u8,
        action: String,
    },
    Kick {
        lobby: String,
        player: String,
    },
    Events,
    ClearEvents,
    Commands,
    Maps,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u32,
    result: Result<Value, String>,
}

pub fn directory(args: &Args) -> PathBuf {
    args.data_dir.clone().unwrap_or_else(|| {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        crate::storage::Store::default_directory(&root)
    })
}

pub fn profile_index(profiles: &crate::profile::Profiles, title: &str) -> Result<usize, String> {
    // A numeric selector is an explicit 1-based index, never an implicit fallback.
    if let Some(index) = title.strip_prefix('#') {
        return index
            .parse::<usize>()
            .ok()
            .and_then(|i| i.checked_sub(1))
            .filter(|i| *i < profiles.items.len())
            .ok_or_else(|| "Unknown profile index (use #1, #2, …).".into());
    }
    let matching: Vec<_> = profiles
        .items
        .iter()
        .enumerate()
        .filter(|(_, p)| p.title == title)
        .map(|(i, _)| i)
        .collect();
    match matching.as_slice() {
        [index] => Ok(*index),
        [] => Err("Unknown profile name. Use profiles to list saved profiles.".into()),
        _ => Err("Ambiguous profile name; use its #index.".into()),
    }
}

pub fn run(args: Args) -> Result<(), String> {
    args.validate()?;
    let directory = directory(&args);
    if let Some(command) = args.command {
        let action = command.action()?;
        #[cfg(windows)]
        match ipc::request(&directory, action.clone()) {
            Ok(value) => return print(value),
            Err(ipc::Failure::Remote(error)) => return Err(error),
            Err(ipc::Failure::Transport(error)) if !action.offline() => return Err(error),
            Err(ipc::Failure::Transport(_)) => {}
        }
        if !action.offline() {
            return Err("No console controller is running. Start with --no-gui.".into());
        }
        // Store's exclusive lock prevents offline CLI writes while either frontend runs.
        let mut session = session::Session::open(&directory)?;
        return print(session.apply(action)?);
    }
    #[cfg(windows)]
    return serve(args, directory);
    #[cfg(not(windows))]
    Err("Warcraft III hosting requires Windows.".into())
}

fn print(value: Value) -> Result<(), String> {
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &value).map_err(|_| "Cannot write console output")?;
    writeln!(output).map_err(|_| "Cannot write console output".into())
}

impl Action {
    fn offline(&self) -> bool {
        matches!(
            self,
            Self::Profiles
                | Self::ProfileGet { .. }
                | Self::ProfileSelect { .. }
                | Self::ProfileNew { .. }
                | Self::ProfileSet { .. }
                | Self::Commands
                | Self::Maps
        )
    }
}

#[cfg(windows)]
fn serve(args: Args, directory: PathBuf) -> Result<(), String> {
    let mut session = session::Session::open(&directory)?;
    session.configure(&args)?;
    let server = ipc::Server::start(&directory)?;
    let _interrupts = security::Interrupts::start()?;
    print(
        json!({"controller":"ready", "headless":session.selected().headless, "profile":session.selected().title}),
    )?;
    if args.host {
        session.apply(Action::Host)?;
    }
    let mut shutdown_at = None;
    let mut listener_error = None;
    loop {
        session.poll();
        if security::interrupted() && !session.quitting {
            let _ = session.apply(Action::Quit);
        }
        let call = if listener_error.is_none() {
            match server.receive() {
                Ok(call) => call,
                Err(error) => {
                    listener_error = Some(error);
                    let _ = session.apply(Action::Quit);
                    None
                }
            }
        } else {
            None
        };
        if let Some((request, reply)) = call {
            let result = if request.version != 1 {
                Err("Unsupported control protocol version".into())
            } else {
                session.apply(request.action)
            };
            let _ = reply.send(Response { version: 1, result });
        }
        if session.quitting {
            let since = shutdown_at.get_or_insert_with(Instant::now);
            if !session.running() && server.idle() {
                return match (listener_error, session.last_result.clone()) {
                    (Some(error), Err(cleanup)) => Err(format!("{error}. {cleanup}")),
                    (Some(error), Ok(())) => Err(error),
                    (None, result) => result,
                };
            }
            if since.elapsed() >= Duration::from_secs(15) {
                return Err(
                    "Shutdown timed out; lobby closure is not confirmed. Inspect Warcraft III."
                        .into(),
                );
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Windows GUI-subsystem executables must attach before parsing console arguments.
pub fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
    }
}

#[cfg(test)]
mod tests;
