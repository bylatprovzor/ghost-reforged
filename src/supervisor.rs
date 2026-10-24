//! Resident watchdog. The controller runs as a separate child in the same
//! interactive Windows session; only that retained child handle can be stopped.
mod policy;
mod state;

#[cfg(windows)]
mod windows;

pub fn run(directory: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    return windows::run(directory);
    #[cfg(not(windows))]
    {
        let _ = directory;
        Err("The resident supervisor requires Windows".into())
    }
}
