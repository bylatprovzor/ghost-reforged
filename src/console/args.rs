use super::Action;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    about = "GHost Reforged — graphical or console Warcraft III hosting",
    version,
    args_conflicts_with_subcommands = false
)]
pub struct Args {
    /// Separate saved profiles and controller endpoint.
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
    /// Exact saved profile title, or #1, #2, ... (one-based index).
    #[arg(long)]
    pub profile: Option<String>,
    /// Select a map inside Documents/Warcraft III/Maps.
    #[arg(long)]
    pub map: Option<PathBuf>,
    /// Start hosting immediately (in either frontend).
    #[arg(long)]
    pub host: bool,
    /// Minimize Warcraft and pause its browser only in an owned idle lobby; disable previews.
    #[arg(long, conflicts_with = "normal")]
    pub headless: bool,
    /// Normal rendering and preview; overrides the saved headless setting.
    #[arg(long, conflicts_with = "headless")]
    pub normal: bool,
    /// Run the local controller without windows, tray, preview, or graphical initialization.
    #[arg(long)]
    pub no_gui: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Args {
    pub fn validate(&self) -> Result<(), String> {
        if self.command.is_some()
            && (self.profile.is_some()
                || self.map.is_some()
                || self.host
                || self.headless
                || self.normal
                || self.no_gui)
        {
            return Err("Startup flags (--profile, --map, --host, --headless, --normal, --no-gui) cannot be combined with a control command. Only --data-dir is shared.".into());
        }
        Ok(())
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Controller state, current lobby and every player's permitted slot actions.
    Status,
    /// Start hosting the selected saved profile on the running controller.
    Host,
    /// Request native lobby closure and stop rehosting (poll status for confirmation).
    Stop,
    /// Force-start the current eligible lobby (poll status/events for confirmation).
    Start,
    /// Stop hosting, close the owned lobby, then exit the console controller.
    Quit,
    /// List saved profile titles, indices and all settings.
    Profiles,
    /// Print a complete saved profile. Omit selector for the selected profile.
    ProfileGet { profile: Option<String> },
    /// Select a profile while stopped.
    ProfileSelect { profile: String },
    /// Create and select a new profile while stopped.
    ProfileNew { title: String },
    /// Merge a JSON object into the saved profile; every hosting option is available.
    /// Live-safe fields apply while hosting. Map/name/counter/speed/privacy require Stop.
    ProfileSet {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, conflicts_with = "file", required_unless_present = "file")]
        json: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Modify a 1-based slot; the expected lobby is mandatory to reject stale commands.
    Slot {
        number: u8,
        #[arg(value_parser=["open","close","observer","computer"])]
        action: String,
        #[arg(long)]
        lobby: String,
    },
    /// Kick an exact full player name from the expected lobby; never a recycled slot number.
    Kick {
        player: String,
        #[arg(long)]
        lobby: String,
    },
    /// Recent operational events (no credentials or raw native frames).
    Events,
    /// Clear the current in-memory activity view (persistent logs are preserved).
    ClearEvents,
    /// List chat commands, descriptions, and which ones this profile enables.
    Commands,
    /// List maps in the Warcraft III maps directory.
    Maps,
}

impl Command {
    pub fn action(self) -> Result<Action, String> {
        Ok(match self {
            Self::Status => Action::Status,
            Self::Host => Action::Host,
            Self::Stop => Action::Stop,
            Self::Start => Action::Start,
            Self::Quit => Action::Quit,
            Self::Profiles => Action::Profiles,
            Self::ProfileGet { profile } => Action::ProfileGet { profile },
            Self::ProfileSelect { profile } => Action::ProfileSelect { profile },
            Self::ProfileNew { title } => Action::ProfileNew { title },
            Self::ProfileSet {
                profile,
                json,
                file,
            } => {
                let text = match (json, file) {
                    (Some(json), None) => json,
                    (None, Some(file)) => {
                        if std::fs::metadata(&file)
                            .map_err(|_| "Cannot read settings file")?
                            .len()
                            > 65536
                        {
                            return Err("Settings file exceeds 64 KiB".into());
                        }
                        std::fs::read_to_string(file).map_err(|_| "Cannot read settings file")?
                    }
                    _ => return Err("Supply exactly one of --json or --file".into()),
                };
                Action::ProfileSet {
                    profile,
                    settings: serde_json::from_str(&text)
                        .map_err(|e| format!("Invalid settings JSON: {e}"))?,
                }
            }
            Self::Slot {
                number,
                action,
                lobby,
            } => Action::Slot {
                number,
                action,
                lobby,
            },
            Self::Kick { player, lobby } => Action::Kick { player, lobby },
            Self::Events => Action::Events,
            Self::ClearEvents => Action::ClearEvents,
            Self::Commands => Action::Commands,
            Self::Maps => Action::Maps,
        })
    }
}
