use super::*;

#[test]
fn explicit_credential_precedence_and_validation() {
    let valid = "a".repeat(43);
    assert_eq!(resolve(None, None).unwrap(), None);
    assert_eq!(
        resolve(Some(valid.clone().into()), Some("missing".into())).unwrap(),
        Some(valid)
    );
    for bad in ["", "short", &"a".repeat(129), &" ".repeat(43)] {
        assert!(resolve(Some(bad.into()), None).is_err());
        assert!(resolve(Some(bad.into()), Some("missing".into())).is_err());
    }
    assert!(resolve(None, Some("relative.dpapi".into())).is_err());
}

#[cfg(windows)]
fn protect(bytes: &[u8]) -> Vec<u8> {
    use std::ptr;
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        assert_ne!(
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output
            ),
            0
        );
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        bytes
    }
}

#[cfg(windows)]
#[test]
fn native_protected_file_survives_reopen_and_rejects_corruption() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("key.dpapi");
    let key = "Synthetic_Native_Test_Credential_0123456789abcdef";
    std::fs::write(&path, protect(key.as_bytes())).unwrap();
    for _ in 0..2 {
        assert_eq!(
            resolve(None, Some(path.clone().into_os_string()))
                .unwrap()
                .as_deref(),
            Some(key)
        );
    }
    for invalid in [
        protect(b"short"),
        protect(&[0xff; 43]),
        b"corrupt".to_vec(),
        vec![0; 8193],
    ] {
        std::fs::write(&path, invalid).unwrap();
        assert!(resolve(None, Some(path.clone().into_os_string())).is_err());
    }
    std::fs::remove_file(&path).unwrap();
    assert!(resolve(None, Some(path.into_os_string())).is_err());
}
