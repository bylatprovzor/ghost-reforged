use super::*;

#[test]
fn a_late_creation_failure_stops_lobby_actions_and_automatic_recreation() {
    for departed in [false, true] {
        let mut engine = creating();
        engine.on_event(&event("MultiplayerGameCreateStarted", json!({})), 1);
        engine.on_event(&event("SetGlueScreen", json!({"screen":"GAME_LOBBY"})), 2);
        engine.on_event(&lobby(vec![player(0, true, false, false)]), 3);
        assert_eq!(engine.phase, Phase::Lobby);
        if departed {
            engine.on_event(&event("MultiplayerGameLeave", json!({})), 4);
            assert!(matches!(engine.phase, Phase::Cooldown(_)));
        }
        assert!(
            engine
                .on_event(
                    &event(
                        "MultiplayerGameCreateResult",
                        json!({"details":{"success":false}})
                    ),
                    5
                )
                .is_empty()
        );
        assert!(
            matches!(&engine.phase, Phase::Stopped(reason) if reason.contains("rejected lobby creation"))
        );
        assert!(engine.owned_lobby(5).is_none());
        assert!(engine.tick(30).is_empty());
        assert!(
            engine
                .on_event(&event("GameList", json!({})), 31)
                .is_empty()
        );
    }
}

#[test]
fn reconnect_cannot_adopt_a_roster_after_its_creation_was_rejected() {
    let mut engine = creating();
    engine.on_event(&event("MultiplayerGameCreateStarted", json!({})), 1);
    engine.recover(2);
    assert!(matches!(engine.phase, Phase::Recovering(_)));
    engine.on_event(
        &event(
            "MultiplayerGameCreateResult",
            json!({"details":{"success":0}}),
        ),
        3,
    );
    assert!(matches!(engine.phase, Phase::Stopped(_)));
    assert!(
        engine
            .on_event(&lobby(vec![player(0, true, false, false)]), 4)
            .is_empty()
    );
    assert!(engine.owned_lobby(4).is_none());
}
