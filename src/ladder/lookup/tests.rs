use super::*;
use serde_json::json;

fn lobby() -> Lobby {
    serde_json::from_value(json!({"isHost":true,"lobbyName":"Ranked #1",
        "mapData":{"mapPath":"ranked.w3x"},"players":[
        {"name":"Player#1234","slot":0,"slotStatus":2,"slotType":0,"team":0,
         "isSelf":false,"isObserver":false,"teamChangeEnabled":true}
    ]}))
    .unwrap()
}

fn pending(lookup: &mut Lookup, now: u64) -> mpsc::Sender<Result<Profile, Error>> {
    let (tx, rx) = mpsc::channel();
    lookup.entries.insert(
        "Player#1234".into(),
        Entry {
            job: Some(rx),
            since: now,
            text: UNKNOWN.into(),
        },
    );
    tx
}

#[test]
fn only_an_explicit_missing_player_is_an_unranked_zero_record() {
    assert_eq!(
        decode(404, r#"{"error":"player_not_found"}"#, "New#1234")
            .unwrap()
            .mmr_summary(),
        "MMR: 1500 (provisional), Rank: ???"
    );
    assert_eq!(
        decode(404, r#"{"error":"player_not_found"}"#, "New#1234")
            .unwrap()
            .summary(),
        "Wins: 0, Loss: 0, Rank: ???"
    );
    for (code, body) in [
        (503, r#"{"error":"service_unavailable"}"#),
        (404, r#"{"error":"not_found"}"#),
        (200, "{}"),
        (200, "[]"),
        (200, "null"),
        (200, "<html>"),
    ] {
        assert!(decode(code, body, "New#1234").is_err());
    }
    let mut value = json!({"battletag":"Player#1234","wins":258,"losses":42,"rank":3});
    assert_eq!(
        decode(200, &value.to_string(), "player#1234")
            .unwrap()
            .summary(),
        "Wins: 258, Loss: 42, Rank: #3"
    );
    assert!(decode(200, &value.to_string(), "Other#1234").is_err());
    value["wins"] = 1.into();
    value["losses"] = 0.into();
    assert!(decode(200, &value.to_string(), "Player#1234").is_err());
}

#[test]
fn pending_reads_are_shared_cached_and_bounded_without_blocking() {
    let mut lookup = Lookup::default();
    lookup.observe(Some(&lobby()));
    let tx = pending(&mut lookup, 0);
    assert!(lookup.query("Player#1234", 0).is_none());
    assert!(lookup.query("Player#1234", 1).is_none());
    assert_eq!(lookup.entries.len(), 1);
    tx.send(Ok(Profile {
        slot: 0,
        battletag: "Player#1234".into(),
        wins: 8,
        losses: 2,
        rank: Some(4),
        mmr: None,
        mmr_games: None,
    }))
    .unwrap();
    assert_eq!(
        lookup.query("Player#1234", 2).unwrap(),
        "Wins: 8, Loss: 2, Rank: #4"
    );
    assert_eq!(
        lookup.query("Player#1234", 61).unwrap(),
        "Wins: 8, Loss: 2, Rank: #4"
    );
    let _pending = pending(&mut lookup, 70);
    assert!(lookup.query("Player#1234", 79).is_none());
    assert_eq!(lookup.query("Player#1234", 80).unwrap(), UNKNOWN);
}

#[test]
fn leaving_or_rehosting_discards_results_and_welcome_waits_for_the_same_record() {
    let roster = lobby();
    let mut lookup = Lookup::default();
    lookup.observe(Some(&roster));
    let tx = pending(&mut lookup, 0);
    let mut welcome = crate::welcome::Announcements::default();
    let settings = crate::welcome::Welcome {
        enabled: true,
        message: "Welcome #{playername}! {stats}".into(),
    };
    welcome.joined("Player#1234");
    assert!(welcome.next(&roster, &settings, 0, &mut lookup).is_none());
    tx.send(Ok(Profile {
        slot: 0,
        battletag: "Player#1234".into(),
        wins: 8,
        losses: 2,
        rank: Some(4),
        mmr: None,
        mmr_games: None,
    }))
    .unwrap();
    assert_eq!(
        welcome
            .next(&roster, &settings, 1, &mut lookup)
            .unwrap()
            .payload["content"],
        "Welcome Player#1234! Wins: 8, Loss: 2, Rank: #4"
    );
    let tx = pending(&mut lookup, 2);
    let mut next = roster.clone();
    next.lobby_name = "Next".into();
    lookup.observe(Some(&next));
    assert!(tx.send(Err(Error::Unavailable)).is_err());
    let tx = pending(&mut lookup, 3);
    next.players.clear();
    lookup.observe(Some(&next));
    assert!(tx.send(Err(Error::Unavailable)).is_err());
    assert_eq!(lookup.query("Player#1234", 4).unwrap(), UNKNOWN);
    assert!(lookup.entries.is_empty());
}

#[test]
fn mmr_summary_is_limited_to_supported_survival_chaos_maps() {
    let mut room = lobby();
    room.map_data.map_name = crate::ladder::MAP_NAME.into();
    let mut lookup = Lookup::default();
    lookup.observe(Some(&room));
    let tx = pending(&mut lookup, 0);
    tx.send(Ok(Profile {
        slot: 0,
        battletag: "Player#1234".into(),
        wins: 0,
        losses: 0,
        rank: None,
        mmr: Some(1000),
        mmr_games: Some(0),
    }))
    .unwrap();
    assert_eq!(
        lookup.query("Player#1234", 1).unwrap(),
        "MMR: 1000 (provisional), Rank: ???"
    );
    room.map_data.map_name = "An unrelated map".into();
    lookup.observe(Some(&room));
    let tx = pending(&mut lookup, 2);
    tx.send(Ok(Profile {
        slot: 0,
        battletag: "Player#1234".into(),
        wins: 2,
        losses: 1,
        rank: None,
        mmr: Some(1024),
        mmr_games: Some(3),
    }))
    .unwrap();
    assert_eq!(
        lookup.query("Player#1234", 3).unwrap(),
        "Wins: 2, Loss: 1, Rank: ???"
    );
}

#[test]
#[ignore = "Read-only check against the deployed player API"]
fn live_individual_profile_read() {
    let profile = fetch_player("ArteMy#21249").expect("Public player API");
    assert_eq!(profile.battletag.to_lowercase(), "artemy#21249");
    println!("{}", profile.announcement());
}

#[test]
fn legacy_map_keeps_legacy_chat_and_clears_the_mmr_cache() {
    let mut room = lobby();
    room.map_data.map_name = crate::ladder::MAP_NAME.into();
    let mut lookup = Lookup::default();
    lookup.observe(Some(&room));
    let tx = pending(&mut lookup, 0);
    room.map_data.map_name = crate::ladder::LEGACY_MAP_NAME.into();
    lookup.observe(Some(&room));
    assert!(tx.send(Err(Error::Unavailable)).is_err());
    assert!(!lookup.mmr);
    let tx = pending(&mut lookup, 1);
    tx.send(Ok(Profile {
        slot: 0,
        battletag: "Player#1234".into(),
        wins: 2,
        losses: 1,
        rank: None,
        mmr: Some(1024),
        mmr_games: Some(3),
    }))
    .unwrap();
    assert_eq!(
        lookup.query("Player#1234", 2).unwrap(),
        "Wins: 2, Loss: 1, Rank: ???"
    );
}
