//! Local operational events; never native session credentials or raw wire frames.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Keep actionable status/command messages locally; never persist wire payloads.
pub fn append(directory: &Path, elapsed: u64, message: &str) {
    let _ = write(directory, elapsed, message);
}

fn write(directory: &Path, elapsed: u64, message: &str) -> std::io::Result<()> {
    let directory = directory.join("logs");
    fs::create_dir_all(&directory)?;
    let current = directory.join("hosting.log");
    if fs::metadata(&current).is_ok_and(|m| m.len() >= 2 * 1024 * 1024) {
        let previous = directory.join("hosting.previous.log");
        if previous.is_file() {
            fs::remove_file(&previous)?;
        }
        fs::rename(&current, previous)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(current)?;
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    writeln!(
        file,
        "unix={unix} elapsed={elapsed}s {}",
        message.replace(['\r', '\n'], " ")
    )
}
