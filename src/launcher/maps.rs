use std::path::{Path, PathBuf};

pub fn downloads_directory() -> Option<PathBuf> {
    documents_directory().map(|documents| preferred(&documents))
}

pub async fn pick() -> Result<Option<PathBuf>, String> {
    let directory =
        downloads_directory().ok_or("Cannot locate the Warcraft III maps directory.")?;
    std::fs::create_dir_all(&directory)
        .map_err(|_| "Cannot open the Warcraft III maps directory.")?;
    rfd::AsyncFileDialog::new()
        .set_directory(directory)
        .set_title("Select a map from Warcraft III / Maps")
        .add_filter("Warcraft III", &["w3x", "w3m"])
        .pick_file()
        .await
        .map(|file| validate(file.path()))
        .transpose()
}

pub fn validate(path: &Path) -> Result<PathBuf, String> {
    let documents =
        documents_directory().ok_or("Cannot locate the Warcraft III maps directory.")?;
    validate_in(path, &documents.join("Warcraft III/Maps"))
}

fn validate_in(path: &Path, maps: &Path) -> Result<PathBuf, String> {
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

fn preferred(documents: &Path) -> PathBuf {
    let maps = documents.join("Warcraft III/Maps");
    [maps.join("Download"), maps.join("Downloads"), maps.clone()]
        .into_iter()
        .find(|path| path.is_dir())
        .unwrap_or(maps)
}

#[cfg(windows)]
fn documents_directory() -> Option<PathBuf> {
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
fn documents_directory() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picker_always_starts_in_game_maps_even_before_the_directory_exists() {
        let documents = tempfile::tempdir().unwrap();
        assert_eq!(
            preferred(documents.path()),
            documents.path().join("Warcraft III/Maps")
        );
        let downloads = documents.path().join("Warcraft III/Maps/Download");
        std::fs::create_dir_all(&downloads).unwrap();
        assert_eq!(preferred(documents.path()), downloads);
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
    fn windows_known_documents_folder_resolves_natively() {
        assert!(documents_directory().is_some_and(|path| path.is_absolute() && path.is_dir()));
    }
}
