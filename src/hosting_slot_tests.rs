use super::*;

fn roster(status: u8, observer: bool) -> Value {
    let mut roster = lobby(false);
    let p = &mut roster["payload"]["players"][1];
    p["slotStatus"] = json!(status);
    p["slotTypeChangeEnabled"] = json!(true);
    p["isObserver"] = json!(observer);
    p["team"] = json!(if observer { 24 } else { 0 });
    p["id"] = json!(42);
    roster
}

fn request(engine: &Engine, action: SlotAction) -> SlotRequest {
    summarize(engine.owned_lobby(1).unwrap(), false, true)
        .slot_actions(1)
        .into_iter()
        .find(|(a, _)| *a == action)
        .unwrap()
        .1
}

#[test]
fn computers_use_the_native_open_slot_command_and_require_acknowledgment() {
    let mut engine = ready(false);
    let mut roster = roster(0, false);
    engine.on_event(&roster, 1);
    let mut pending = slots::PendingSlot::default();
    pending.request(request(&engine, SlotAction::Computer), &engine, 1);
    assert!(pending.advance(&engine, 1, true, false, true).0.is_empty());
    let (query, _) = pending.advance(&engine, 1, false, true, true);
    assert_eq!(query[0].message, "SendGameLobbySetup");
    assert!(pending.advance(&engine, 2, true, false, true).0.is_empty());
    let (commands, activity) = pending.advance(&engine, 2, false, true, true);
    assert!(activity.is_none());
    assert_eq!(commands[0].message, "SetComputerSlot");
    assert_eq!(commands[0].payload, json!({"slot":1,"difficulty":1}));
    roster["payload"]["players"][1]["slotStatus"] = json!(2);
    roster["payload"]["players"][1]["slotType"] = json!(1);
    engine.on_event(&roster, 3);
    assert_eq!(
        pending.advance(&engine, 3, true, false, true).1.as_deref(),
        Some("Slot 2 · Add computer · applied")
    );
    assert!(pending.advance(&engine, 3, false, true, true).0.is_empty());
}

#[test]
fn an_observer_slot_can_be_opened_but_the_native_ui_does_not_support_observer_ai() {
    let mut engine = ready(false);
    engine.on_event(&roster(1, true), 1);
    let snapshot = summarize(engine.owned_lobby(1).unwrap(), false, true);
    let choices = snapshot.slot_actions(1);
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].0, SlotAction::Open);
}

#[test]
fn observer_moves_require_native_team_permission_and_a_free_observer_slot() {
    let mut engine = ready(true);
    let mut roster = roster(2, false);
    let mut observer = roster["payload"]["players"][1].clone();
    observer["slot"] = json!(2);
    observer["slotStatus"] = json!(0);
    observer["isObserver"] = json!(true);
    observer["team"] = json!(24);
    roster["payload"]["players"]
        .as_array_mut()
        .unwrap()
        .push(observer);
    engine.on_event(&roster, 1);
    let mut pending = slots::PendingSlot::default();
    pending.request(request(&engine, SlotAction::Observer), &engine, 1);
    pending.advance(&engine, 1, false, true, true);
    pending.advance(&engine, 2, true, false, true);
    let commands = pending.advance(&engine, 2, false, true, true).0;
    assert_eq!(commands[0].message, "SetTeam");
    assert_eq!(commands[0].payload, json!({"slot":1,"team":24}));
    roster["payload"]["players"][1]["teamChangeEnabled"] = json!(false);
    engine.on_event(&roster, 3);
    let snapshot = summarize(engine.owned_lobby(3).unwrap(), false, true);
    assert!(
        !snapshot
            .slot_actions(1)
            .iter()
            .any(|(a, _)| *a == SlotAction::Observer)
    );
}

#[test]
fn a_join_or_countdown_invalidates_a_queued_computer_request() {
    for countdown in [false, true] {
        let mut engine = ready(false);
        engine.on_event(&roster(0, false), 1);
        let mut pending = slots::PendingSlot::default();
        pending.request(request(&engine, SlotAction::Computer), &engine, 1);
        pending.advance(&engine, 1, false, true, true);
        engine.on_event(&roster(2, false), 2);
        if countdown {
            engine.start_now(2);
        }
        let (commands, activity) = pending.advance(&engine, 2, true, false, true);
        assert!(commands.is_empty());
        assert!(activity.unwrap().ends_with("not applied"));
    }
}
