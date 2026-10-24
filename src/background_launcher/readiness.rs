//! Startup evidence for a launcher started without a game request. These markers
//! permit sending --exec; they never prove that Warcraft has authenticated.
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const READ_LIMIT: u64 = 512 * 1024;

#[derive(Default)]
pub(super) struct Generation(Option<(u32, u64, Startup)>);

impl Generation {
    pub fn ready(&mut self, pid: u32, created: u64) -> bool {
        self.startup(pid, created).is_some_and(Startup::ready)
    }

    fn startup(&mut self, pid: u32, created: u64) -> Option<&mut Startup> {
        if self
            .0
            .as_ref()
            .is_none_or(|(old_pid, old_created, _)| (*old_pid, *old_created) != (pid, created))
        {
            let ticks = created.checked_sub(116_444_736_000_000_000)?;
            let since = UNIX_EPOCH.checked_add(Duration::from_nanos(ticks.checked_mul(100)?))?;
            self.0 = Some((pid, created, Startup::new(since)));
        }
        self.0.as_mut().map(|(_, _, startup)| startup)
    }
}

pub(super) struct Startup {
    since: SystemTime,
    files: Vec<(PathBuf, u64, bool)>,
}

impl Startup {
    pub fn new(since: SystemTime) -> Self {
        Self {
            since,
            files: Vec::new(),
        }
    }

    pub fn ready(&mut self) -> bool {
        for root in ["LOCALAPPDATA", "APPDATA"]
            .into_iter()
            .filter_map(std::env::var_os)
        {
            if self.in_directory(&PathBuf::from(root).join("Battle.net/Logs")) {
                return true;
            }
        }
        false
    }

    fn in_directory(&mut self, root: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(root) else {
            return false;
        };
        let mut files: Vec<_> = entries
            .take(1024)
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                if !name.starts_with("battle.net-") || !name.ends_with(".log") {
                    return None;
                }
                let metadata = entry.metadata().ok()?;
                let created = metadata.created().ok()?;
                (metadata.is_file() && created >= self.since)
                    .then(|| (created, entry.path(), metadata.len()))
            })
            .collect();
        files.sort_unstable_by_key(|(created, _, _)| *created);
        for (_, path, _) in files.into_iter().take(16) {
            let Ok(file) = File::open(&path) else {
                continue;
            };
            // NTFS directory entries can retain the old size while Battle.net
            // keeps writing. Inspect the open file before consulting the cache.
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            let length = metadata.len();
            if let Some((_, previous_length, ready)) =
                self.files.iter().find(|(file, _, _)| *file == path)
                && *previous_length == length
            {
                if *ready {
                    return true;
                }
                continue;
            }
            let mut bytes = Vec::new();
            if file.take(READ_LIMIT).read_to_end(&mut bytes).is_err() {
                continue;
            }
            let ready = initialized(&bytes);
            self.files.retain(|(file, _, _)| *file != path);
            if self.files.len() >= 16 {
                self.files.remove(0);
            }
            self.files.push((path, length, ready));
            if ready {
                return true;
            }
        }
        false
    }
}

fn initialized(bytes: &[u8]) -> bool {
    let mut controller = false;
    let mut frontend = false;
    let mut search = false;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        // A partial log write cannot be evidence of a completed transition.
        if line.last() != Some(&b'\n') {
            continue;
        }
        let line = String::from_utf8_lossy(line);
        controller |=
            line.contains("[GameController] {Main} GameController initialization complete");
        search |= line.contains(
            "[StartupGameSearch] {Main} Completed automated search for unregistered games",
        );
        if line.contains("[Frontend] {Main} Attempting to show main window.") {
            frontend = line
                .split_whitespace()
                .any(|field| field == "isExpectingLogin=false");
        }
    }
    controller && frontend && search
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const CONTROLLER: &str =
        "I date [GameController] {Main} GameController initialization complete\n";
    const FRONTEND: &str =
        "I date [Frontend] {Main} Attempting to show main window. isExpectingLogin=false\n";
    const SEARCH: &str =
        "I date [StartupGameSearch] {Main} Completed automated search for unregistered games\n";

    #[test]
    fn complete_initialization_requires_all_markers_and_no_sign_in_prompt() {
        let complete = format!("{CONTROLLER}{FRONTEND}{SEARCH}");
        assert!(initialized(complete.as_bytes()));
        for partial in [
            format!("{CONTROLLER}{FRONTEND}"),
            format!("{FRONTEND}{SEARCH}"),
            format!("{CONTROLLER}{SEARCH}"),
            complete.trim_end().into(),
            complete.replace("isExpectingLogin=false", "isExpectingLogin=true"),
        ] {
            assert!(!initialized(partial.as_bytes()));
        }
        assert!(!initialized(
            format!("{complete}{}", FRONTEND.replace("false", "true")).as_bytes()
        ));
    }

    #[test]
    fn a_fresh_file_can_become_ready_but_old_logs_cannot_authorize_a_launch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("battle.net-fixture.log");
        let mut file = File::create(&path).unwrap();
        file.write_all(format!("{CONTROLLER}{FRONTEND}").as_bytes())
            .unwrap();
        file.flush().unwrap();
        let born = file.metadata().unwrap().created().unwrap();
        let mut startup = Startup::new(born);
        assert!(!startup.in_directory(directory.path()));
        assert!(!startup.in_directory(directory.path()));
        file.write_all(SEARCH.as_bytes()).unwrap();
        file.flush().unwrap();
        assert!(startup.in_directory(directory.path()));
        let mut newer = Startup::new(born + std::time::Duration::from_secs(1));
        assert!(!newer.in_directory(directory.path()));
    }

    #[test]
    fn a_replaced_launcher_cannot_reuse_cached_readiness_even_with_the_same_pid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("battle.net-fixture.log");
        std::fs::write(&path, format!("{CONTROLLER}{FRONTEND}{SEARCH}")).unwrap();
        let born = path.metadata().unwrap().created().unwrap();
        let ticks = 116_444_736_000_000_000
            + born.duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64 / 100;
        let mut generation = Generation::default();
        assert!(
            generation
                .startup(7, ticks)
                .unwrap()
                .in_directory(directory.path())
        );
        assert!(
            !generation
                .startup(7, ticks + 10_000_000)
                .unwrap()
                .in_directory(directory.path())
        );
        // A new controller can adopt the original lifetime without trusting its age.
        let mut adopted = Generation::default();
        assert!(
            adopted
                .startup(7, ticks)
                .unwrap()
                .in_directory(directory.path())
        );
    }
}
