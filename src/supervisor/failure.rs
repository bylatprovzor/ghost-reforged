use std::{io::Write, path::Path, time::SystemTime};

pub(super) fn record(directory: &Path, controller: bool, error: &str) -> std::io::Result<()> {
    let receipt = serde_json::json!({
        "version": 1,
        "unix_seconds": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs(),
        "pid": std::process::id(),
        "controller": controller,
        "error": error.chars().take(1024).collect::<String>(),
    });
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, &receipt)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(directory.join("supervisor-startup-error.json"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_failures_replace_the_previous_receipt_and_remain_bounded() {
        let directory = tempfile::tempdir().unwrap();
        record(directory.path(), false, "First failure").unwrap();
        record(directory.path(), true, &"é".repeat(2048)).unwrap();
        let bytes = std::fs::read(directory.path().join("supervisor-startup-error.json")).unwrap();
        assert!(bytes.len() < 4096);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["pid"], std::process::id());
        assert_eq!(value["controller"], true);
        assert_eq!(value["error"].as_str().unwrap().chars().count(), 1024);
    }
}
