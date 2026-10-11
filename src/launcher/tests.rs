use super::{
    app::{App, Field, Message},
    profile::{Profile, Profiles},
    storage::{StorageError, Store},
};
use std::fs;

#[test]
fn reservation_is_durable_before_creation_and_cannot_be_reused() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path(), None);
    app.reserve_number(1).unwrap();
    assert!(app.reserve_number(1).is_err());
    drop(app);
    let app = App::new(dir.path(), None);
    assert_eq!(app.form.number, "2");
}

#[test]
fn failed_reservation_preserves_the_current_number() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path(), None);
    fs::create_dir(dir.path().join("profiles.json")).unwrap();
    assert!(app.reserve_number(1).is_err());
    assert_eq!(app.form.number, "1");
    assert_eq!(app.profiles.items[0].next_number, 1);
}

#[test]
fn profile_roundtrip_preserves_counter_and_rejects_a_second_writer() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut profiles) = Store::open(dir.path()).unwrap();
    assert!(matches!(Store::open(dir.path()), Err(StorageError::Locked)));
    profiles.items[0].next_number = 42;
    store.save(&profiles).unwrap();
    drop(store);
    let (store, mut profiles) = Store::open(dir.path()).unwrap();
    assert_eq!(profiles.items[0].next_number, 42);
    profiles.items[0].next_number = 43;
    store.save(&profiles).unwrap();
    drop(store);
    let (_, profiles) = Store::open(dir.path()).unwrap();
    assert_eq!(profiles.items[0].next_number, 43);
}

#[test]
fn corrupt_and_future_profiles_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    fs::write(&path, "broken!").unwrap();
    assert!(matches!(Store::open(dir.path()), Err(StorageError::Format)));
    assert_eq!(fs::read_to_string(&path).unwrap(), "broken!");
    let future = Profiles {
        version: 2,
        ..Profiles::default()
    };
    let bytes = serde_json::to_vec(&future).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        Store::open(dir.path()),
        Err(StorageError::Invalid(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn failed_save_does_not_replace_the_in_memory_profile() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path(), None);
    fs::create_dir(dir.path().join("profiles.json")).unwrap();
    let _ = app.update(Message::Edit(Field::Number, "42".into()));
    let _ = app.update(Message::Save);
    assert!(app.notice_error);
    assert_eq!(app.profiles.items[0].next_number, 1);
}

#[test]
fn invalid_profiles_cannot_be_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let (store, mut profiles) = Store::open(dir.path()).unwrap();
    profiles.items[0].bot_name = "Person#1234".into();
    assert!(store.save(&profiles).is_err());
    profiles.items[0].bot_name = "Ghost".into();
    profiles.items[0].lobby_pattern = "\u{e9}".repeat(11) + "{n}";
    assert!(store.save(&profiles).is_err());
    profiles.items[0].lobby_pattern = "SC #{n}".into();
    profiles.selected = 10;
    assert!(store.save(&profiles).is_err());
}

#[test]
fn new_profile_and_switch_save_edits_without_losing_existing_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path(), None);
    let _ = app.update(Message::Edit(Field::Number, "42".into()));
    let _ = app.update(Message::Edit(Field::Bot, "Ghost++".into()));
    let _ = app.update(Message::NewProfile);
    assert_eq!(app.profiles.selected, 1);
    assert_eq!(app.profiles.items[0].next_number, 42);
    let _ = app.update(Message::SelectProfile(0));
    assert_eq!(app.form.number, "42");
    assert_eq!(app.form.profile.bot_name, "Ghost++");
}

#[test]
fn invalid_numeric_input_does_not_fall_back_or_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path(), None);
    let _ = app.update(Message::Edit(Field::Number, "NaN".into()));
    let _ = app.update(Message::Save);
    assert!(app.notice_error);
    assert!(!dir.path().join("profiles.json").exists());
}

#[test]
fn unknown_imported_fields_are_rejected_and_map_path_is_kept_local() {
    let profile = Profile::default();
    let mut value = serde_json::to_value(&profile).unwrap();
    value["credentials"] = serde_json::json!("do not import");
    assert!(serde_json::from_value::<Profile>(value).is_err());
    let mut invalid = profile;
    invalid.map = Some("not-a-map.exe".into());
    assert!(invalid.validate().is_err());
}

#[test]
fn existing_profiles_keep_their_counter_after_the_product_rename() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        Store::default_directory(directory.path()),
        directory.path().join("GHostReforged")
    );
    let old = directory.path().join("GHostReborn");
    let (store, mut profiles) = Store::open(&old).unwrap();
    profiles.items[0].next_number = 42;
    store.save(&profiles).unwrap();
    drop(store);
    assert_eq!(Store::default_directory(directory.path()), old);
    let app = App::new(&Store::default_directory(directory.path()), None);
    assert_eq!(app.form.number, "42");
}

#[test]
fn hosting_settings_survive_profile_selection_and_restart() {
    use super::profile::GameSpeed;
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    let _ = app.update(Message::Speed(GameSpeed::Slow));
    let _ = app.update(Message::RequireFull(true));
    let _ = app.update(Message::Edit(Field::RehostDelay, "45".into()));
    let _ = app.update(Message::Edit(Field::EmptyRefresh, "600".into()));
    let _ = app.update(Message::NewProfile);
    let _ = app.update(Message::SelectProfile(0));
    assert_eq!(app.form.profile.speed, GameSpeed::Slow);
    assert!(app.form.profile.require_full);
    assert_eq!(app.form.rehost_delay, "45");
    drop(app);
    let app = App::new(directory.path(), None);
    assert_eq!(app.form.empty_refresh, "600");
    assert_eq!(app.form.profile.speed, GameSpeed::Slow);
}

#[test]
fn old_profiles_load_new_settings_without_changing_saved_values() {
    let mut value = serde_json::to_value(Profile::default()).unwrap();
    for key in [
        "speed",
        "require_full",
        "rehost_delay_seconds",
        "empty_rehost_seconds",
    ] {
        value.as_object_mut().unwrap().remove(key);
    }
    value["next_number"] = serde_json::json!(19);
    let profile: Profile = serde_json::from_value(value).unwrap();
    assert_eq!(profile.next_number, 19);
    assert_eq!(profile.rehost_delay_seconds, 15);
    assert_eq!(profile.empty_rehost_seconds, 0);
}

#[test]
fn map_selection_generates_the_default_name_and_respects_a_custom_override() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    let _ = app.update(Message::Picked(Some(
        "5473_SurvivalChaosReborn_v1.59_fix1.w3x".into(),
    )));
    assert_eq!(app.form.profile.lobby_pattern, "Survival Chaos #{n}");
    let _ = app.update(Message::Edit(Field::Pattern, "Friends #{n}".into()));
    let _ = app.update(Message::Picked(Some("CastleFight.w3x".into())));
    assert_eq!(app.form.profile.lobby_pattern, "Friends #{n}");
    let _ = app.update(Message::MapName);
    assert_eq!(app.form.profile.lobby_pattern, "Castle Fight #{n}");
    assert!(app.form.profile.automatic_name);
}

#[test]
fn preview_modal_requires_an_image_and_blocks_covered_controls_until_closed() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    let _ = app.update(Message::OpenPreview);
    assert!(!app.preview.expanded);
    app.preview.image = Some(iced::widget::image::Handle::from_rgba(1, 1, vec![255; 4]));
    let _ = app.update(Message::OpenPreview);
    assert!(app.preview.expanded);
    assert!(app.preview.image.is_some());
    let _ = app.update(Message::Edit(Field::Bot, "Changed".into()));
    assert_eq!(app.form.profile.bot_name, "Ghost");
    let _ = app.update(Message::ClosePreview);
    assert!(!app.preview.expanded);
    let _ = app.update(Message::Edit(Field::Bot, "Changed".into()));
    assert_eq!(app.form.profile.bot_name, "Changed");
}
