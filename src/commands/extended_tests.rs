use super::*;

fn text(outcome: Outcome) -> String {
    outcome.reply.unwrap().payload["content"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn yes_joins_only_an_existing_unambiguous_vote_with_distinct_voters() {
    let mut commands = LobbyCommands::default();
    let settings = CommandSettings::default();
    commands.observe(Some(&lobby()), 0);
    assert!(text(commands.handle(&chat("Player1", "?yes"), &settings, 1)).contains("No matching"));
    commands.handle(&chat("Player1", "?votestart"), &settings, 2);
    assert!(
        !commands
            .handle(&chat("Player1", "?yes"), &settings, 3)
            .start
    );
    commands.handle(&chat("Player1", "?votekick Player3"), &settings, 4);
    assert!(text(commands.handle(&chat("Player2", "?yes"), &settings, 5)).contains("Two votes"));
    assert!(
        commands
            .handle(&chat("Player2", "?yes KICK"), &settings, 6)
            .kick
            .is_some()
    );
    assert!(
        commands
            .handle(&chat("Player2", "?yes START"), &settings, 7)
            .start
    );
    commands.observe(Some(&lobby()), 62);
    assert!(text(commands.handle(&chat("Player2", "?yes"), &settings, 62)).contains("No matching"));
    commands.handle(&chat("Player1", "?votestart"), &settings, 63);
    let mut disabled = settings;
    disabled.set(ChatCommand::VoteStart, false);
    commands.apply_settings(&disabled);
    assert!(text(commands.handle(&chat("Player2", "?yes"), &disabled, 64)).contains("No matching"));
}

#[test]
fn commands_from_different_players_in_the_same_millisecond_both_respond() {
    let mut commands = LobbyCommands::default();
    let settings = CommandSettings::default();
    commands.observe(Some(&lobby()), 0);
    for player in ["Player1", "Player2", "Player4"] {
        assert!(
            commands
                .handle_at(&chat(player, "?help"), &settings, 1000, Context::default())
                .reply
                .is_some()
        );
    }
    assert!(
        commands
            .handle_at(
                &chat("Player1", "?map"),
                &settings,
                1199,
                Context::default()
            )
            .reply
            .is_none()
    );
    assert!(
        commands
            .handle_at(
                &chat("Player1", "?map"),
                &settings,
                1200,
                Context::default()
            )
            .reply
            .is_some()
    );
    let mut other_room = chat("Player3", "?map");
    other_room["payload"]["message"]["channelName"] = json!("Another lobby");
    assert!(
        commands
            .handle_at(&other_room, &settings, 1200, Context::default())
            .reply
            .is_none()
    );
}

#[test]
fn region_map_and_help_use_available_data_without_exposing_paths() {
    let mut roster = lobby();
    roster.players[0].name.clear();
    roster.players[1].player_region = "eu".into();
    roster.players[2].gateway_id = Some(10);
    roster.players[3].name = "Twin#3".into();
    roster.players[4].name = "Twin#4".into();
    roster.map_data.map_name = "Survival Chaos v1.59".into();
    roster.map_data.map_path = "C:\\Users\\PrivateUser\\Maps\\Survival v1.59.w3x".into();
    let mut commands = LobbyCommands::default();
    commands.observe(Some(&roster), 0);
    let settings = CommandSettings::default();
    let response = text(commands.handle(&chat("Player1", "?from"), &settings, 1));
    assert!(response.contains("Player1: Europe") && response.contains("Player2: Americas"));
    assert!(response.contains("Twin#3: unknown") && !response.contains("Computer"));
    assert!(
        text(commands.handle(&chat("Player1", "?from Twin"), &settings, 2)).contains("ambiguous")
    );
    assert!(
        text(commands.handle(&chat("Player1", "?from Twin#3"), &settings, 3))
            .contains("Twin#3: unknown")
    );
    let response = text(commands.handle(&chat("Player1", "?map"), &settings, 4));
    assert!(response.contains("Survival Chaos v1.59") && response.contains("Survival v1.59.w3x"));
    assert!(!response.contains("PrivateUser"));
    assert!(
        text(commands.handle(&chat("Player1", "?help yes"), &settings, 5)).contains("?yes start")
    );
}

#[test]
fn colors_respect_map_constraints_and_revalidate_native_identity() {
    let mut roster = lobby();
    roster.players[1].color = Some(0);
    roster.players[1].color_change_enabled = true;
    roster.players[2].color = Some(1);
    let (reply, denied) = color::request(&roster, "Player1", "blue");
    assert!(denied.is_none() && reply.contains("unavailable"));
    let (_, request) = color::request(&roster, "Player1", "light-blue");
    let request = request.unwrap();
    assert_eq!(
        request.confirm(&roster).unwrap().payload,
        json!({"slot":1,"color":9})
    );
    roster.players[1].id = Some(91);
    assert!(request.confirm(&roster).is_none());
    roster.players[1].id = Some(1);
    roster.players[1].color_change_enabled = false;
    assert!(request.confirm(&roster).is_none());
    roster.players[1].color_change_enabled = true;
    roster.is_custom_players = true;
    roster.available_team_colors = json!([[0, 2], [1]]);
    assert!(request.confirm(&roster).is_none());
    assert!(color::request(&roster, "Player1", "teal").1.is_some());
    roster.available_team_colors = json!({"0":[0,2], "1":[1]});
    assert!(color::request(&roster, "Player1", "teal").1.is_some());
    roster.players[1].is_observer = true;
    assert!(color::request(&roster, "Player1", "teal").1.is_none());
}

#[test]
fn optional_placeholder_metadata_never_discards_the_essential_roster() {
    let value = json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"test.w3x","mapName":null},
        "players":[{"slot":1,"slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,"teamChangeEnabled":true,
          "playerRegion":null,"gatewayId":"pending","color":255,"colorChangeEnabled":null,"name":"Player1"}]});
    let roster: Lobby = serde_json::from_value(value).unwrap();
    assert_eq!(roster.humans(false), 1);
    assert!(roster.players[0].player_region.is_empty());
    assert!(roster.players[0].gateway_id.is_none());
    assert!(color::request(&roster, "Player1", "blue").1.is_none());
    assert!(info::regions(&roster, "Player1").contains("unknown"));
}
