use super::*;
use serde_json::json;

fn lobby(name: &str) -> Lobby {
    serde_json::from_value(json!({"isHost":true,"lobbyName":name,"mapData":{"mapPath":"test.w3x"},"players":[
        {"name":"","id":0,"slot":0,"slotStatus":2,"slotType":0,"team":24,"isSelf":true,"isObserver":true,"teamChangeEnabled":true},
        {"name":"Player#1","id":1,"slot":1,"slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,"teamChangeEnabled":true},
        {"name":"Watcher#2","id":2,"slot":2,"slotStatus":2,"slotType":0,"team":24,"isSelf":false,"isObserver":true,"teamChangeEnabled":true},
        {"name":"Computer","id":3,"slot":3,"slotStatus":2,"slotType":1,"team":1,"isSelf":false,"isObserver":false,"teamChangeEnabled":false}]})).unwrap()
}

#[test]
fn history_survives_restart_and_counts_only_confirmed_playing_humans_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("players.json");
    {
        let mut history = History::open(Some(&path));
        let mut tracker = Tracker::default();
        tracker.observe(&lobby("Test1"), &mut history, 1_791_676_800);
        assert!(history.describe("Player#1").contains("0 game(s)"));
        tracker.started("Test1", &mut history, 1_791_676_810);
        tracker.started("Test1", &mut history, 1_791_676_811);
        tracker.observe(&lobby("Test2"), &mut history, 1_791_676_820);
        // A cancelled countdown in Test2 never calls started.
        assert!(history.describe("Player#1").contains("1 game(s)"));
        assert!(history.describe("Watcher#2").contains("0 game(s)"));
        assert_eq!(history.book.players.len(), 2);
    }
    let history = History::open(Some(&path));
    assert!(!history.unavailable);
    assert!(history.describe("PLAYER#1").contains("1 game(s)"));
    assert_eq!(history.book.players["player#1"].first_seen, 1_791_676_800);
    assert_eq!(history.book.players["player#1"].last_seen, 1_791_676_820);
}

#[test]
fn corrupt_history_and_a_second_writer_are_preserved_without_blocking_hosting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("players.json");
    std::fs::write(&path, "broken history").unwrap();
    let mut history = History::open(Some(&path));
    history.seen("Player#1", 1, true);
    assert!(history.describe("Player#1").contains("unavailable"));
    assert!(
        history
            .resolve("Player#1")
            .unwrap_err()
            .contains("unavailable")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken history");
    drop(history);
    let other_path = dir.path().join("other.json");
    let _first = History::open(Some(&other_path));
    let second = History::open(Some(&other_path));
    assert!(second.unavailable);
}

#[test]
fn names_need_unique_matches_and_dates_are_utc() {
    let mut history = History::default();
    history.seen("Twin#1", 0, false);
    history.seen("Twin#2", 1, false);
    assert!(history.resolve("twin").is_err());
    assert_eq!(history.resolve("TWIN#1").unwrap(), "Twin#1");
    assert_eq!(date(0), "1970-01-01");
    assert_eq!(date(951_782_400), "2000-02-29");
    assert_eq!(date(1_791_676_800), "2026-10-11");
}

#[test]
fn transient_blank_profiles_keep_stats_but_a_replacement_does_not_inherit_them() {
    let mut history = History::default();
    let mut tracker = Tracker::default();
    let mut roster = lobby("Test1");
    tracker.observe(&roster, &mut history, 100);
    roster.players[1].name.clear();
    tracker.observe(&roster, &mut history, 200);
    tracker.started("Test1", &mut history, 201);
    assert_eq!(history.book.players["player#1"].games, 1);
    assert_eq!(history.book.players["watcher#2"].last_seen, 200);
    roster.lobby_name = "Test2".into();
    roster.players[1].name = "Player#1".into();
    tracker.observe(&roster, &mut history, 300);
    roster.players[1].name.clear();
    roster.players[1].id = Some(99);
    tracker.observe(&roster, &mut history, 301);
    tracker.started("Test2", &mut history, 302);
    assert_eq!(history.book.players["player#1"].games, 1);
}

#[test]
fn failed_disk_write_keeps_in_memory_stats_and_retries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("players.json");
    let mut history = History::open(Some(&path));
    std::fs::create_dir(&path).unwrap();
    let began = std::time::Instant::now();
    history.seen("Player#1", 1, true);
    assert!(began.elapsed() < std::time::Duration::from_millis(250));
    assert!(history.describe("Player#1").contains("1 game(s)"));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while history.warning().is_none() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::fs::remove_dir(&path).unwrap();
    drop(history); // Flush retries the existing pending record without another join.
    assert!(
        History::open(Some(&path))
            .describe("Player#1")
            .contains("1 game(s)")
    );
}
