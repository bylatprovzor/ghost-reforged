use std::path::{Path, PathBuf};

pub async fn pick() -> Result<Option<PathBuf>, String> {
    let documents =
        documents_directory().ok_or("Cannot locate the Warcraft III maps directory.")?;
    let directory = prepare_directory(&documents.join("Warcraft III/Maps"))?;
    let (sender, receiver) = iced::futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("ghost-map-picker".into())
        .spawn(move || {
            let _ = sender.send(native_pick(&directory));
        })
        .map_err(|_| "Cannot open the Warcraft III map picker.")?;
    receiver.await.map_err(|_| "The map picker stopped.")?
}

#[cfg(windows)]
unsafe extern "C" {
    fn ghost_pick_map(root: *const u16, output: *mut u16, capacity: u32) -> i32;
    #[cfg(test)]
    fn ghost_map_dialog_probe(root: *const u16) -> i32;
}

#[cfg(windows)]
fn shell_path(path: &Path) -> Result<Vec<u16>, String> {
    let path = path.to_str().ok_or("The map directory path is invalid.")?;
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        return Ok(format!(r"\\{unc}").encode_utf16().chain(Some(0)).collect());
    }
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    Ok(path.encode_utf16().chain(Some(0)).collect())
}

#[cfg(windows)]
fn native_pick(directory: &Path) -> Result<Option<PathBuf>, String> {
    let root = shell_path(directory)?;
    let mut output = vec![0u16; 32768];
    let result = unsafe { ghost_pick_map(root.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    match result {
        0 => {
            let length = output
                .iter()
                .position(|unit| *unit == 0)
                .ok_or("Invalid selected map path.")?;
            let path = PathBuf::from(
                String::from_utf16(&output[..length]).map_err(|_| "Invalid selected map path.")?,
            );
            validate(&path).map(Some)
        }
        1 => Ok(None),
        _ => Err(format!(
            "Cannot open the restricted Warcraft III map picker ({result:#x})."
        )),
    }
}

#[cfg(not(windows))]
fn native_pick(_: &Path) -> Result<Option<PathBuf>, String> {
    Err("Map selection requires Windows.".into())
}

fn prepare_directory(directory: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory)
        .map_err(|_| "Cannot open the Warcraft III maps directory.")?;
    // Windows Shell silently ignores mixed slash paths that filesystem APIs accept.
    directory
        .canonicalize()
        .map_err(|_| "Cannot resolve the Warcraft III maps directory.".into())
}

use ghost_reforged::maps::documents_directory;
pub use ghost_reforged::maps::validate;
#[cfg(test)]
use ghost_reforged::maps::validate_in;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picker_creates_only_the_game_maps_directory_when_missing() {
        let documents = tempfile::tempdir().unwrap();
        let maps = documents.path().join("Warcraft III/Maps");
        assert_eq!(
            prepare_directory(&maps).unwrap(),
            maps.canonicalize().unwrap()
        );
        assert!(!documents.path().join("Downloads").exists());
    }

    #[test]
    fn selection_rejects_downloads_sibling_directories_and_parent_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let maps = directory.path().join("Maps");
        let inside = maps.join("Download/Test.w3x");
        let outside = directory.path().join("Downloads/Test.w3x");
        let sibling = directory.path().join("Maps-old/Test.w3x");
        for file in [&inside, &outside, &sibling] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, b"fixture").unwrap();
        }
        assert_eq!(
            validate_in(&inside, &maps).unwrap(),
            inside.canonicalize().unwrap()
        );
        assert!(validate_in(&outside, &maps).is_err());
        assert!(validate_in(&sibling, &maps).is_err());
        assert!(validate_in(&maps.join("../Downloads/Test.w3x"), &maps).is_err());
        assert!(validate_in(&maps.join("Download"), &maps).is_err());
        let text = maps.join("not-a-map.txt");
        std::fs::write(&text, b"fixture").unwrap();
        assert!(validate_in(&text, &maps).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn picker_directory_is_accepted_and_locked_by_the_native_windows_shell() {
        let directory = tempfile::tempdir().unwrap();
        let prepared = prepare_directory(&directory.path().join("Warcraft III/Maps")).unwrap();
        let root = shell_path(&prepared).unwrap();
        assert_eq!(unsafe { ghost_map_dialog_probe(root.as_ptr()) }, 0);
    }
    #[cfg(windows)]
    #[test]
    fn windows_known_documents_folder_resolves_natively() {
        assert!(documents_directory().is_some_and(|path| path.is_absolute() && path.is_dir()));
    }
}
