//! Host-only credentials, never part of a map, profile, or operational event.
use super::Error;
use std::{ffi::OsString, path::Path};

pub(crate) fn read() -> Result<Option<String>, Error> {
    resolve(
        std::env::var_os("W3LADDER_COLLECTOR_KEY"),
        std::env::var_os("W3LADDER_COLLECTOR_KEY_FILE"),
    )
}

fn validate(value: String) -> Result<String, Error> {
    if (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(value)
    } else {
        Err(Error::Unavailable)
    }
}

fn resolve(key: Option<OsString>, file: Option<OsString>) -> Result<Option<String>, Error> {
    // Preserve the explicit process override. A malformed override must not
    // silently authorize requests using a different stored credential.
    if let Some(key) = key {
        return validate(key.into_string().map_err(|_| Error::Unavailable)?).map(Some);
    }
    let Some(file) = file else { return Ok(None) };
    let path = Path::new(&file);
    if !path.is_absolute() {
        return Err(Error::Unavailable);
    }
    read_protected(path).and_then(validate).map(Some)
}

#[cfg(not(windows))]
fn read_protected(_: &Path) -> Result<String, Error> {
    Err(Error::Unavailable)
}

#[cfg(windows)]
fn read_protected(path: &Path) -> Result<String, Error> {
    use std::{fs::File, io::Read, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };
    let mut encrypted = Vec::new();
    File::open(path)
        .and_then(|f| f.take(8193).read_to_end(&mut encrypted))
        .map_err(|_| Error::Unavailable)?;
    if encrypted.is_empty() || encrypted.len() > 8192 {
        return Err(Error::Unavailable);
    }
    let source = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI recognizes both current-user and local-machine blobs. Never prompt
    // from a background task; a missing user profile is a recoverable failure.
    unsafe {
        if CryptUnprotectData(
            &source,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        ) == 0
        {
            return Err(Error::Unavailable);
        }
        let value = if output.pbData.is_null() || !(43..=128).contains(&output.cbData) {
            Err(Error::Unavailable)
        } else {
            std::str::from_utf8(std::slice::from_raw_parts(
                output.pbData,
                output.cbData as usize,
            ))
            .map(str::to_owned)
            .map_err(|_| Error::Unavailable)
        };
        if !output.pbData.is_null() {
            for offset in 0..output.cbData as usize {
                ptr::write_volatile(output.pbData.add(offset), 0);
            }
            LocalFree(output.pbData.cast());
        }
        value
    }
}

#[cfg(test)]
mod tests;
