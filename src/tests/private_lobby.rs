use super::*;

#[test]
fn private_creation_requires_a_native_valid_password() {
    let directory = tempfile::tempdir().unwrap();
    let map = directory.path().join("test.w3x");
    std::fs::write(&map, []).unwrap();
    let mut config = Config {
        maps: vec![map],
        private_game: true,
        ..Config::default()
    };
    for password in ["", "ab", "twenty-characters---x", "test\nvalue"] {
        config.lobby_password = password.into();
        assert!(
            config.validate().is_err(),
            "Invalid private lobby was accepted"
        );
    }
    config.lobby_password = "private-test".into();
    assert!(config.validate().is_ok());
    let command = protocol::create_lobby(&config, 1, 0);
    assert_eq!(command.payload["privateGame"], true);
    assert_eq!(command.payload["password"], "private-test");
}

#[test]
fn legacy_private_profile_remains_editable_but_cannot_host_without_a_password() {
    let mut encoded = serde_json::to_value(crate::profile::Profile::default()).unwrap();
    encoded.as_object_mut().unwrap().remove("lobby_password");
    let mut profile: crate::profile::Profile = serde_json::from_value(encoded).unwrap();
    assert!(profile.validate().is_ok());
    assert!(
        profile
            .config(std::path::Path::new("."))
            .unwrap_err()
            .contains("lobby password")
    );
    profile.lobby_password = "private-test".into();
    assert!(profile.validate().is_ok());
}
