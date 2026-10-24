use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    hosting: bool,
    #[serde(default)]
    quit: bool,
}

pub(super) fn read(directory: &Path) -> Result<bool, String> {
    Ok(load(directory)?.hosting)
}

pub(super) fn quitting(directory: &Path) -> Result<bool, String> {
    Ok(load(directory)?.quit)
}

fn load(directory: &Path) -> Result<Intent, String> {
    let bytes = match std::fs::read(directory.join("hosting-intent.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Intent {
                version: 1,
                ..Intent::default()
            });
        }
        Err(_) => return Err("Cannot read saved hosting intent".into()),
    };
    if bytes.len() > 1024 {
        return Err("Invalid saved hosting intent; automatic start is disabled".into());
    }
    let intent: Intent = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid saved hosting intent; automatic start is disabled")?;
    if intent.version != 1 {
        return Err("Unsupported hosting intent version; automatic start is disabled".into());
    }
    Ok(intent)
}

pub(super) fn save(directory: &Path, hosting: bool) -> Result<(), String> {
    persist(directory, hosting, false)
}

pub(super) fn quit(directory: &Path) -> Result<(), String> {
    persist(directory, false, true)
}

fn persist(directory: &Path, hosting: bool, quit: bool) -> Result<(), String> {
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(
            &mut file,
            &Intent {
                version: 1,
                hosting,
                quit,
            },
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(directory.join("hosting-intent.json"))?;
        Ok(())
    };
    write().map_err(|_| "Cannot save hosting intent".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_preserves_stop_and_never_guesses_after_corruption() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!read(dir.path()).unwrap());
        save(dir.path(), true).unwrap();
        assert!(read(dir.path()).unwrap());
        save(dir.path(), false).unwrap();
        assert!(!read(dir.path()).unwrap());
        std::fs::write(dir.path().join("hosting-intent.json"), b"{").unwrap();
        assert!(read(dir.path()).is_err());
    }
}
