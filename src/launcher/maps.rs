use std::path::{Path, PathBuf};

pub fn downloads_directory() -> Option<PathBuf> {
    documents_directory().map(|documents| preferred(&documents))
}

fn preferred(documents: &Path) -> PathBuf {
    let maps = documents.join("Warcraft III/Maps");
    [
        maps.join("Download"),
        maps.join("Downloads"),
        maps,
        documents.to_path_buf(),
    ]
    .into_iter()
    .find(|path| path.is_dir())
    .unwrap_or_else(|| documents.to_path_buf())
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
    fn downloaded_maps_are_preferred_with_existing_parent_fallback() {
        let documents = tempfile::tempdir().unwrap();
        assert_eq!(preferred(documents.path()), documents.path());
        let downloads = documents.path().join("Warcraft III/Maps/Download");
        std::fs::create_dir_all(&downloads).unwrap();
        assert_eq!(preferred(documents.path()), downloads);
    }
    #[cfg(windows)]
    #[test]
    fn windows_known_documents_folder_resolves_natively() {
        assert!(documents_directory().is_some_and(|path| path.is_absolute() && path.is_dir()));
    }
}
