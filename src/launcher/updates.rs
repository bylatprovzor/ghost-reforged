use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = "https://github.com/bylatprovzor/ghost-reforged";
const API: &str = "https://api.github.com/repos/bylatprovzor/ghost-reforged/releases?per_page=30";
const SETUP: &str = "GHost-Reforged-Setup-x64.exe";

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub preview: bool,
    url: String,
    hash: String,
    size: u64,
}

#[derive(Debug, Clone)]
pub struct Download {
    pub directory: PathBuf,
    pub hash: String,
}

#[derive(Default)]
pub struct State {
    pub available: Option<Release>,
    pub downloading: bool,
    pub ready: Option<Download>,
    pub resume: bool,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct Published {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
    state: String,
}

fn version(value: &str) -> Option<[u32; 3]> {
    let parts: Vec<_> = value
        .strip_prefix('v')
        .unwrap_or(value)
        .split('.')
        .collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some([
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ])
}

pub(super) fn select(bytes: &[u8], current: &str) -> Result<Option<Release>, String> {
    let releases: Vec<Published> =
        serde_json::from_slice(bytes).map_err(|_| "GitHub returned an invalid release list.")?;
    let current = version(current).ok_or("The installed version is invalid.")?;
    Ok(releases
        .into_iter()
        .filter_map(|r| {
            let next = version(&r.tag_name)?;
            if r.draft || next <= current {
                return None;
            }
            let expected = format!("{REPOSITORY}/releases/download/{}/{SETUP}", r.tag_name);
            let asset = r.assets.into_iter().find(|a| {
                a.name == SETUP
                    && a.state == "uploaded"
                    && a.browser_download_url == expected
                    && (1..=256 * 1024 * 1024).contains(&a.size)
            })?;
            let hash = asset.digest?.strip_prefix("sha256:")?.to_owned();
            if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            Some((
                next,
                Release {
                    version: r.tag_name,
                    preview: r.prerelease,
                    url: expected,
                    hash,
                    size: asset.size,
                },
            ))
        })
        .max_by_key(|(v, _)| *v)
        .map(|(_, release)| release))
}

async fn background<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("ghost-update".into())
        .spawn(move || {
            let _ = tx.send(job());
        })
        .map_err(|_| "The update check could not start.")?;
    rx.await.map_err(|_| "The update check was interrupted.")?
}

pub async fn check() -> Result<Option<Release>, String> {
    background(check_once).await
}

fn check_once() -> Result<Option<Release>, String> {
    let directory = tempfile::tempdir().map_err(|_| "The update cache is unavailable.")?;
    let path = directory.path().join("releases.json");
    fetch(API, &path, 12, 2 * 1024 * 1024)?;
    let bytes = std::fs::read(path).map_err(|_| "The release list could not be read.")?;
    select(&bytes, VERSION)
}

pub async fn download(release: Release) -> Result<Download, String> {
    background(move || {
        let directory = tempfile::Builder::new()
            .prefix("ghost-update-")
            .tempdir()
            .map_err(|_| "The update cache is unavailable.")?;
        let path = directory.path().join(SETUP);
        fetch(&release.url, &path, 180, release.size)?;
        verify(&path, &release.hash, release.size)?;
        std::fs::write(
            directory.path().join("update.ps1"),
            include_str!("update.ps1"),
        )
        .map_err(|_| "The update helper could not be saved.")?;
        Ok(Download {
            directory: directory.keep(),
            hash: release.hash,
        })
    })
    .await
}

#[cfg(windows)]
fn fetch(url: &str, path: &Path, seconds: u32, limit: u64) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let curl = PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows is unavailable.")?)
        .join("System32/curl.exe");
    let status = std::process::Command::new(curl)
        .args([
            "--disable",
            "--silent",
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "8",
            "--max-time",
        ])
        .arg(seconds.to_string())
        .arg("--max-filesize")
        .arg(limit.to_string())
        .args([
            "--user-agent",
            "GHost-Reforged",
            "--header",
            "Accept: application/vnd.github+json",
            "--output",
        ])
        .arg(path)
        .arg(url)
        .creation_flags(0x08000000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| "Windows could not contact GitHub.")?;
    if !status.success() {
        return Err("GitHub is unavailable. Hosting is unaffected.".into());
    }
    Ok(())
}

#[cfg(windows)]
fn verify(path: &Path, expected: &str, size: u64) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|_| "The update could not be read.")?;
    if file
        .metadata()
        .map_err(|_| "The update could not be read.")?
        .len()
        != size
    {
        return Err("The update download is incomplete. Please retry.".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "The update could not be read.")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected) {
        return Err(
            "The update checksum does not match GitHub. Installation was cancelled.".into(),
        );
    }
    Ok(())
}

#[cfg(windows)]
pub fn install(download: &Download, data: &Path, resume: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let app = std::env::current_exe().map_err(|_| "The application location is unavailable.")?;
    let powershell =
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows is unavailable.")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = std::process::Command::new(powershell);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(download.directory.join("update.ps1"))
        .arg("-WaitPid")
        .arg(std::process::id().to_string())
        .arg("-Application")
        .arg(app)
        .arg("-DataDirectory")
        .arg(data)
        .arg("-ExpectedHash")
        .arg(&download.hash)
        .creation_flags(0x08000000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if resume {
        command.arg("-ResumeHosting");
    }
    command
        .spawn()
        .map_err(|_| "The installer could not start. Please retry.")?;
    Ok(())
}

#[cfg(not(windows))]
fn fetch(_: &str, _: &Path, _: u32, _: u64) -> Result<(), String> {
    Err("Updates require Windows.".into())
}
#[cfg(not(windows))]
fn verify(_: &Path, _: &str, _: u64) -> Result<(), String> {
    Err("Updates require Windows.".into())
}
#[cfg(not(windows))]
pub fn install(_: &Download, _: &Path, _: bool) -> Result<(), String> {
    Err("Updates require Windows.".into())
}

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;
