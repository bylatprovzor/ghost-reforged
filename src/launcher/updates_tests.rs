use super::*;
use serde_json::json;

fn release(tag: &str) -> serde_json::Value {
    json!({"tag_name":tag,"draft":false,"prerelease":true,"assets":[{
        "name":SETUP,"state":"uploaded","size":123,
        "browser_download_url":format!("{REPOSITORY}/releases/download/{tag}/{SETUP}"),
        "digest":format!("sha256:{}", "a".repeat(64))
    }]})
}

#[test]
fn published_preview_versions_are_compared_numerically_and_never_downgrade() {
    let releases = serde_json::to_vec(&vec![
        release("v0.2.9"),
        release("v0.2.10"),
        release("v0.2.3"),
    ])
    .unwrap();
    assert_eq!(
        select(&releases, "0.2.8").unwrap().unwrap().version,
        "v0.2.10"
    );
    assert_eq!(select(&releases, "0.2.10").unwrap(), None);
    assert_eq!(select(&releases, "1.0.0").unwrap(), None);
}

#[test]
fn untrusted_or_incomplete_releases_cannot_offer_an_installer() {
    for field in ["draft", "url", "digest", "state", "version", "size"] {
        let mut bad = release("v0.2.4");
        match field {
            "draft" => bad["draft"] = json!(true),
            "url" => {
                bad["assets"][0]["browser_download_url"] = json!("https://example.com/setup.exe")
            }
            "digest" => bad["assets"][0]["digest"] = json!(null),
            "state" => bad["assets"][0]["state"] = json!("new"),
            "version" => bad["tag_name"] = json!("v9.0.0/../../run"),
            "size" => bad["assets"][0]["size"] = json!(9999999999u64),
            _ => unreachable!(),
        }
        assert_eq!(
            select(&serde_json::to_vec(&vec![bad]).unwrap(), "0.2.3").unwrap(),
            None,
            "{field}"
        );
    }
    assert!(select(b"not json", "0.2.3").is_err());
}

#[test]
#[cfg(windows)]
fn installer_bytes_must_match_both_the_size_and_github_digest() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SETUP);
    std::fs::write(&path, b"installer fixture").unwrap();
    let hash = format!("{:x}", Sha256::digest(b"installer fixture"));
    assert!(verify(&path, &hash, 17).is_ok());
    assert!(verify(&path, &hash, 18).is_err());
    std::fs::write(&path, b"changed installer").unwrap();
    assert!(verify(&path, &hash, 17).is_err());
}

#[test]
fn failed_or_deferred_update_checks_do_not_change_hosting_intent() {
    use crate::app::{App, Message};
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path(), None);
    app.hosting_status = "Lobby open".into();
    let _ = app.update(Message::UpdateChecked(Err("offline".into())));
    assert!(app.updates.available.is_none());
    let available = select(
        &serde_json::to_vec(&vec![release("v9.0.0")]).unwrap(),
        "0.2.3",
    )
    .unwrap();
    let _ = app.update(Message::UpdateChecked(Ok(available)));
    assert!(app.updates.available.is_some());
    let _ = app.update(Message::DismissUpdate);
    assert!(app.updates.available.is_none());
    assert_eq!(app.hosting_status, "Lobby open");
}

#[test]
#[ignore = "contacts the public GitHub API"]
fn startup_check_works_with_the_published_github_release_list() {
    assert!(check_once().is_ok());
}
