use super::profile::Profiles;
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Profile storage is unavailable.")]
    Io(#[from] std::io::Error),
    #[error("Another launcher is using this profile directory.")]
    Locked,
    #[error("The profile file could not be read. It has not been overwritten.")]
    Format,
    #[error("{0}")]
    Invalid(String),
}

pub struct Store {
    path: PathBuf,
    _lock: File,
}

impl Store {
    pub fn default_directory(root: &Path) -> PathBuf {
        let legacy = root.join("GHostReborn");
        let current = root.join("GHostReforged");
        // Keep one writer and the saved counter when upgrading an existing install.
        if !current.join("profiles.json").is_file() && legacy.join("profiles.json").is_file() {
            legacy
        } else {
            current
        }
    }
    pub fn open(directory: &Path) -> Result<(Self, Profiles), StorageError> {
        fs::create_dir_all(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("launcher.lock"))?;
        FileExt::try_lock_exclusive(&lock).map_err(|_| StorageError::Locked)?;
        let store = Self {
            path: directory.join("profiles.json"),
            _lock: lock,
        };
        let mut profiles = match fs::read(&store.path) {
            Ok(bytes) => {
                serde_json::from_slice::<Profiles>(&bytes).map_err(|_| StorageError::Format)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Profiles::default(),
            Err(error) => return Err(error.into()),
        };
        if profiles.version == 1 {
            for profile in &mut profiles.items {
                profile
                    .commands
                    .0
                    .extend(ghost_reforged::commands::ChatCommand::NEW);
            }
            profiles.version = 2;
        }
        profiles.validate().map_err(StorageError::Invalid)?;
        Ok((store, profiles))
    }

    pub fn save(&self, profiles: &Profiles) -> Result<(), StorageError> {
        profiles.validate().map_err(StorageError::Invalid)?;
        let bytes = serde_json::to_vec_pretty(profiles).map_err(|_| StorageError::Format)?;
        let directory = self.path.parent().expect("store has a directory");
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| StorageError::Io(error.error))?;
        Ok(())
    }
}
