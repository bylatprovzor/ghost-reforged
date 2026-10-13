//! Map validation used by both frontends; paths remain inside the game's map directory.
use std::path::{Path, PathBuf};
pub fn validate(path: &Path) -> Result<PathBuf, String> {
    let documents =
        documents_directory().ok_or("Cannot locate the Warcraft III maps directory.")?;
    validate_in(path, &documents.join("Warcraft III/Maps"))
}

pub fn validate_in(path: &Path, maps: &Path) -> Result<PathBuf, String> {
    let selected = path
        .canonicalize()
        .map_err(|_| "The selected map is unavailable.")?;
    let root = maps
        .canonicalize()
        .map_err(|_| "The Warcraft III maps directory is unavailable.")?;
    if !selected.starts_with(root) {
        return Err("Select a map inside Documents / Warcraft III / Maps.".into());
    }
    if !selected.is_file()
        || !selected.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("w3x") || extension.eq_ignore_ascii_case("w3m")
        })
    {
        return Err("Select a Warcraft III map (.w3x or .w3m).".into());
    }
    Ok(selected)
}

#[cfg(windows)]
pub fn documents_directory() -> Option<PathBuf> {
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath},
    };
    let mut path = std::ptr::null_mut();
    unsafe {
        let result = SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut path);
        if result < 0 || path.is_null() {
            CoTaskMemFree(path.cast());
            return None;
        }
        let mut length = 0;
        while *path.add(length) != 0 {
            length += 1;
        }
        let directory = PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(
            path, length,
        )));
        CoTaskMemFree(path.cast());
        Some(directory)
    }
}

#[cfg(not(windows))]
pub fn documents_directory() -> Option<PathBuf> {
    None
}
