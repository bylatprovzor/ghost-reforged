//! Retained ownership of a newly requested, verified client. Listener readiness
//! ends pre-API recovery; a caller may first transfer the claim to login recovery.
//! Existing sessions are never claimed.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub(crate) struct Startup {
    requested: Option<(u64, Instant)>,
    #[cfg(windows)]
    client: Option<crate::background_render::process::Process>,
}

impl Startup {
    pub fn requested(&mut self) {
        *self = Self::default();
        let ticks = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            / 100;
        self.requested = Some((116_444_736_000_000_000 + ticks as u64, Instant::now()));
    }

    pub fn ready(&mut self) {
        *self = Self::default();
    }

    pub fn expired(&self, budget: Duration) -> bool {
        self.requested.is_some_and(|(_, at)| at.elapsed() >= budget)
    }

    #[cfg(windows)]
    pub fn observe(&mut self, pid: u32) {
        use crate::background_render::process::{Process, processes};
        let Some((since, _)) = self.requested else {
            return;
        };
        if self.client.is_some() {
            return;
        }
        let Some(root) = std::env::var_os("ProgramFiles(x86)") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let expected = root.join("Warcraft III/_retail_/x86_64/Warcraft III.exe");
        let launcher = root.join("Battle.net/Battle.net.exe");
        let Ok(rows) = processes() else { return };
        let Some((_, parent, _)) = rows.iter().find(|row| row.0 == pid) else {
            return;
        };
        let Ok(parent) = Process::open(*parent, false) else {
            return;
        };
        if !crate::background_render::process::same_path(&parent.path, &launcher)
            && parent.pid != std::process::id()
        {
            return;
        }
        self.client = claim(pid, since, &expected, &parent);
    }

    #[cfg(windows)]
    pub fn owns(&self, pid: u32) -> bool {
        self.client
            .as_ref()
            .is_some_and(|client| client.pid == pid && client.running())
    }

    #[cfg(not(windows))]
    pub fn observe(&mut self, _: u32) {}
    #[cfg(not(windows))]
    pub fn owns(&self, _: u32) -> bool {
        false
    }
}

#[cfg(windows)]
fn claim(
    pid: u32,
    since: u64,
    expected: &std::path::Path,
    parent: &crate::background_render::process::Process,
) -> Option<crate::background_render::process::Process> {
    use crate::background_render::process::{Process, same_path};
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    let client = Process::open(pid, false).ok()?;
    let mut own_session = 0;
    let mut client_session = 0;
    if unsafe { ProcessIdToSessionId(std::process::id(), &mut own_session) } == 0
        || unsafe { ProcessIdToSessionId(pid, &mut client_session) } == 0
        || own_session == 0
        || client_session != own_session
        || client.created < since
        || !same_path(&client.path, expected)
        || !client.child_of(parent).ok()?
    {
        return None;
    }
    Some(client)
}

#[cfg(all(test, windows))]
pub(crate) fn fixture_claim(pid: u32) -> Startup {
    let parent =
        crate::background_render::process::Process::open(std::process::id(), false).unwrap();
    Startup {
        client: Some(claim(pid, 0, &std::env::current_exe().unwrap(), &parent).unwrap()),
        ..Default::default()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::background_render::process::Process;
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };

    #[test]
    fn startup_claim_requires_new_matching_child_and_is_revoked_on_readiness() {
        let parent = Process::open(std::process::id(), false).unwrap();
        let path = std::env::current_exe().unwrap();
        let mut startup = Startup::default();
        assert!(!startup.owns(parent.pid));
        assert!(!startup.expired(Duration::ZERO));
        startup.requested();
        let mut child = Command::new(&path)
            .args([
                "--exact",
                "background_priority::tests::priority_fixture",
                "--nocapture",
            ])
            .env("GHOST_PRIORITY_FIXTURE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let since = startup.requested.unwrap().0;
        let owned = claim(child.id(), since, &path, &parent).unwrap();
        assert!(claim(child.id(), owned.created + 1, &path, &parent).is_none());
        assert!(claim(child.id(), since, &path.with_extension("other"), &parent).is_none());
        assert!(claim(child.id(), since, &path, &owned).is_none());
        startup.client = Some(owned);
        assert!(startup.owns(child.id()));
        assert!(!startup.owns(parent.pid));
        assert!(startup.expired(Duration::ZERO));
        startup.ready();
        assert!(!startup.owns(child.id()));
        assert!(!startup.expired(Duration::ZERO));
        child.stdin.take();
        assert!(child.wait().unwrap().success());
    }
}
