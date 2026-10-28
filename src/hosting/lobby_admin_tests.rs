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

#[test]
fn a_reserved_slot_closes_opens_when_full_and_is_kept_for_its_player() {
    let mut a = admin();
    let lobby = room(&["Boss#1", "Guest#2"]);
    a.observe(&lobby, 0);
    let (_, actions, _) = a
        .handle(&say("Boss#1", "?reserve Friend"), &lobby, 1)
        .unwrap();
    assert_eq!(actions, vec![Action::Close(4)]);
    assert!(a.holds_start());
    // Slot 4 closed; slot 3 still free: nothing to do.
    let mut lobby = room(&["Boss#1", "Guest#2"]);
    lobby.players[4].slot_status = 1;
    assert!(a.observe(&lobby, 5).0.is_empty());
    // Slot 3 taken: the lobby is otherwise full, so open the reserved slot.
    lobby.players[3].slot_status = 2;
    lobby.players[3].name = "Third#3".into();
    assert_eq!(a.observe(&lobby, 6).0, vec![Action::Open(4)]);
    // Someone else takes it: kicked.
    lobby.players[4].slot_status = 2;
    lobby.players[4].name = "Stranger#4".into();
    let (actions, lines) = a.observe(&lobby, 10);
    assert_eq!(actions, vec![Action::Kick(4)]);
    assert!(lines[0].contains("reserved for Friend"));
    // The reserved player arrives: the reservation ends.
    lobby.players[4].name = "Friend#77".into();
    let (actions, lines) = a.observe(&lobby, 14);
    assert!(actions.is_empty() && lines[0].contains("Friend is here"));
    assert!(!a.holds_start());
}

#[test]
fn open_cancels_the_reservation_and_the_admin_leaving_does_too() {
    let mut a = admin();
    let lobby = room(&["Boss#1"]);
    a.observe(&lobby, 0);
    a.handle(&say("Boss#1", "?reserve Friend#9"), &lobby, 1)
        .unwrap();
    let mut closed = room(&["Boss#1"]);
    closed.players[4].slot_status = 1;
    let (_, actions, _) = a.handle(&say("Boss#1", "?open"), &closed, 2).unwrap();
    assert_eq!(actions, vec![Action::Open(4)]);
    // The start waits until Warcraft shows the slot open again.
    assert!(a.holds_start());
    a.observe(&lobby, 2);
    assert!(!a.holds_start());
    a.handle(&say("Boss#1", "?reserve Friend#9"), &lobby, 3)
        .unwrap();
    let mut gone = room(&[]);
    gone.players[4].slot_status = 1;
    let (actions, _) = a.observe(&gone, 4);
    assert_eq!(actions, vec![Action::Open(4)]);
    assert!(a.holds_start());
    // Still closed later: the bot asks again.
    assert_eq!(a.observe(&gone, 10).0, vec![Action::Open(4)]);
    a.observe(&room(&[]), 11);
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
