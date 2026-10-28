use super::*;
use serde_json::json;

/// Bot in slot 0 as observer; four playing slots 1..=4, the given humans in order.
fn room(humans: &[&str]) -> Lobby {
    let mut players = vec![
        json!({"name":"Bot#1","slot":0,"slotStatus":2,"slotType":0,"team":24,
        "isSelf":true,"isObserver":true,"teamChangeEnabled":true}),
    ];
    for slot in 1..=4u8 {
        let human = humans.get(slot as usize - 1);
        players.push(json!({"name":human.copied().unwrap_or(""),"slot":slot,
            "slotStatus":if human.is_some() {2} else {0},"slotType":0,"team":slot - 1,
            "isSelf":false,"isObserver":false,"teamChangeEnabled":true}));
    }
    serde_json::from_value(json!({"isHost":true,"lobbyName":"Rank #1",
        "mapData":{"mapPath":"map.w3x"},"players":players}))
    .unwrap()
}

fn say(sender: &str, text: &str) -> Value {
    json!({"messageType":"ChatMessage","payload":{"message":{"source":"gameChat",
        "sender":sender,"content":text,"channelName":"Rank #1"}}})
}

fn admin() -> LobbyAdmin {
    LobbyAdmin {
        admins: Admins::with(&["Boss#1"]),
        ..LobbyAdmin::default()
    }
}

#[test]
fn only_admins_kick_and_admins_cannot_be_kicked() {
    let lobby = room(&["Boss#1", "Guest#2", "Other#3"]);
    let mut a = admin();
    a.observe(&lobby, 0);
    let (reply, actions, _) = a.handle(&say("Guest#2", "?kick Other"), &lobby, 1).unwrap();
    assert!(actions.is_empty() && reply.starts_with("Only lobby admins"));
    let (_, actions, _) = a.handle(&say("Boss#1", "?kick Other"), &lobby, 2).unwrap();
    assert_eq!(actions, vec![Action::Kick(3)]);
    let (reply, actions, _) = a.handle(&say("Boss#1", "!kick Boss#1"), &lobby, 3).unwrap();
    assert!(actions.is_empty() && reply.contains("cannot be kicked"));
    assert!(a.handle(&say("Boss#1", "kick Other"), &lobby, 4).is_none());
    assert!(
        a.handle(&say("Boss#1", "?votekick Other"), &lobby, 4)
            .is_none()
    );
}

#[test]
fn stop_holds_until_go_or_the_admin_leaves() {
    let mut lobby = room(&["Boss#1", "Guest#2"]);
    let mut a = admin();
    a.observe(&lobby, 0);
    a.handle(&say("Boss#1", "?stop"), &lobby, 1).unwrap();
    assert!(a.holds_start());
    a.handle(&say("Boss#1", "?go"), &lobby, 2).unwrap();
    assert!(!a.holds_start());
    a.handle(&say("Boss#1", "?stop"), &lobby, 3).unwrap();
    lobby = room(&["Guest#2"]);
    let (_, lines) = a.observe(&lobby, 4);
    assert!(!a.holds_start());
    assert!(lines[0].contains("auto start is back on"));
}

/// Seat `slot` (1..=4) taken by `name`.
fn join(lobby: &mut Lobby, slot: usize, name: &str) {
    lobby.players[slot].slot_status = 2;
    lobby.players[slot].name = name.into();
}

fn leave(lobby: &mut Lobby, slot: usize) {
    lobby.players[slot].slot_status = 0;
    lobby.players[slot].name = String::new();
}

#[test]
fn a_held_seat_never_touches_slots_and_the_last_arrival_makes_room() {
    let mut a = admin();
    let mut lobby = room(&["Boss#1", "Guest#2"]);
    a.observe(&lobby, 0);
    let (reply, actions, _) = a
        .handle(&say("Boss#1", "?reserve Friend"), &lobby, 1)
        .unwrap();
    assert!(actions.is_empty() && reply.contains("Holding a seat for Friend"));
    assert!(a.holds_start());
    // Seat 4 fills first, then seat 3: the lobby is full, and the player who
    // joined last (seat 3, not the highest seat) is the one asked to leave.
    join(&mut lobby, 4, "Third#3");
    assert!(a.observe(&lobby, 5).0.is_empty());
    join(&mut lobby, 3, "Fourth#4");
    let (actions, lines) = a.observe(&lobby, 6);
    assert_eq!(actions, vec![Action::Kick(3)]);
    assert!(lines[0].contains("Fourth#4: the last seat is kept for Friend"));
    // One request at a time while Warcraft applies the kick.
    assert!(a.observe(&lobby, 7).0.is_empty());
    leave(&mut lobby, 3);
    assert!(a.observe(&lobby, 10).0.is_empty());
    assert!(a.holds_start());
    // The awaited player arrives: the reservation ends and the start may go.
    join(&mut lobby, 3, "Friend#77");
    let (actions, lines) = a.observe(&lobby, 11);
    assert!(actions.is_empty() && lines[0].contains("Friend is here"));
    assert!(!a.holds_start());
}

#[test]
fn a_full_lobby_with_an_admin_last_is_told_once_and_nobody_is_kicked() {
    let mut a = LobbyAdmin {
        admins: Admins::with(&["Boss#1", "Mod#5"]),
        ..LobbyAdmin::default()
    };
    let mut lobby = room(&["Boss#1", "Guest#2", "Third#3"]);
    a.observe(&lobby, 0);
    a.handle(&say("Boss#1", "?reserve Friend"), &lobby, 1)
        .unwrap();
    join(&mut lobby, 4, "Mod#5");
    let (actions, lines) = a.observe(&lobby, 5);
    assert!(actions.is_empty() && lines[0].contains("?open to start without Friend"));
    assert!(a.observe(&lobby, 10).1.is_empty());
    assert!(a.holds_start());
    let (reply, actions, _) = a.handle(&say("Boss#1", "?open"), &lobby, 11).unwrap();
    assert!(actions.is_empty() && reply.contains("no longer held"));
    assert!(!a.holds_start());
}

#[test]
fn open_cancels_the_reservation_and_the_admin_leaving_does_too() {
    let mut a = admin();
    let lobby = room(&["Boss#1"]);
    a.observe(&lobby, 0);
    a.handle(&say("Boss#1", "?reserve Friend#9"), &lobby, 1)
        .unwrap();
    let (reply, _, _) = a
        .handle(&say("Boss#1", "?reserve Other#8"), &lobby, 2)
        .unwrap();
    assert!(reply.contains("already held for Friend#9"));
    let (reply, actions, _) = a.handle(&say("Boss#1", "?open"), &lobby, 2).unwrap();
    assert!(actions.is_empty() && reply.contains("Friend#9 is no longer held"));
    assert!(!a.holds_start());
    let (reply, _, _) = a.handle(&say("Boss#1", "?open"), &lobby, 3).unwrap();
    assert!(reply.contains("No seat is held"));
    a.handle(&say("Boss#1", "?reserve Friend#9"), &lobby, 3)
        .unwrap();
    let (actions, lines) = a.observe(&room(&[]), 4);
    assert!(actions.is_empty() && lines[0].contains("Boss#1 left"));
    assert!(!a.holds_start());
}

#[test]
fn a_full_lobby_cannot_hold_a_seat() {
    let mut a = admin();
    let lobby = room(&["Boss#1", "Guest#2", "Third#3", "Fourth#4"]);
    a.observe(&lobby, 0);
    let (reply, actions, _) = a
        .handle(&say("Boss#1", "?reserve Friend"), &lobby, 1)
        .unwrap();
    assert!(actions.is_empty() && reply.contains("The lobby is full"));
    assert!(!a.holds_start());
}

#[test]
fn admin_lists_skip_comments_and_names_without_a_tag() {
    let list = parse_list("\u{feff}Boss#1\n; note\n\n  other#22  \nnotag\n");
    assert_eq!(
        list,
        ["boss#1", "other#22"]
            .map(String::from)
            .into_iter()
            .collect()
    );
}
