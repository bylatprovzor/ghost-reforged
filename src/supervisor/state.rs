use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub struct State {
    directory: PathBuf,
    _lock: File,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerIdentity {
    pub version: u32,
    pub pid: u32,
    pub born: u64,
}

impl State {
    pub fn worker(&self) -> Result<Option<WorkerIdentity>, String> {
        let bytes = match std::fs::read(self.directory.join("supervisor-worker.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("Cannot read retained worker identity".into()),
        };
        if bytes.len() > 1024 {
            return Err("Invalid retained worker identity".into());
        }
        let value: WorkerIdentity =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid retained worker identity")?;
        if value.version != 1 || value.pid == 0 || value.born == 0 {
            return Err("Invalid retained worker identity".into());
        }
        Ok(Some(value))
    }

    pub fn retain_worker(&self, worker: &WorkerIdentity) -> Result<(), String> {
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
            serde_json::to_writer(&mut file, worker)?;
            file.flush()?;
            file.as_file().sync_all()?;
            file.persist(self.directory.join("supervisor-worker.json"))?;
            Ok(())
        };
        write().map_err(|_| "Cannot retain worker identity".into())
    }
    pub fn open(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory).map_err(|_| "Supervisor storage is unavailable")?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("supervisor.lock"))
            .map_err(|_| "Cannot open supervisor lock")?;
        FileExt::try_lock_exclusive(&file).map_err(|_| "Another supervisor is already running")?;
        Ok(Self {
            directory: directory.to_owned(),
            _lock: file,
        })
    }

    pub fn record(
        &self,
        event: &'static str,
        worker_pid: Option<u32>,
        restarts: u32,
    ) -> Result<(), String> {
        #[derive(Serialize)]
        struct Receipt {
            version: u32,
            unix_seconds: u64,
            supervisor_pid: u32,
            worker_pid: Option<u32>,
            event: &'static str,
            restarts: u32,
        }
        let receipt = Receipt {
            version: 1,
            unix_seconds: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            supervisor_pid: std::process::id(),
            worker_pid,
            event,
            restarts,
        };
        let write = || -> Result<(), Box<dyn std::error::Error>> {
            let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
            serde_json::to_writer(&mut file, &receipt)?;
            file.flush()?;
            file.persist(self.directory.join("supervisor-status.json"))?;
            let log = self.directory.join("supervisor.jsonl");
            if std::fs::metadata(&log).is_ok_and(|m| m.len() >= 262144) {
                let previous = self.directory.join("supervisor.previous.jsonl");
                if previous.exists() {
                    std::fs::remove_file(&previous)?;
                }
                std::fs::rename(&log, previous)?;
            }
            let mut file = OpenOptions::new().append(true).create(true).open(log)?;
            serde_json::to_writer(&mut file, &receipt)?;
            writeln!(file)?;
            Ok(())
        };
        write().map_err(|_| "Cannot write supervisor status".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_excludes_a_second_supervisor_and_logs_remain_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let state = State::open(directory.path()).unwrap();
        assert!(State::open(directory.path()).is_err());
        let log = directory.path().join("supervisor.jsonl");
        std::fs::write(&log, vec![b' '; 262144]).unwrap();
        state.record("idle", None, 0).unwrap();
        assert!(std::fs::metadata(log).unwrap().len() < 1024);
        drop(state);
        assert!(State::open(directory.path()).is_ok());
    }
}
