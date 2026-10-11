use super::*;

fn lobby() -> Lobby {
    serde_json::from_value(
        json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
        "players":(0..6).map(|id| json!({"id":id,"name":format!("Player{id}"),"slot":id,
        "slotStatus":2,"slotType":if id==5 {1}else{0},"team":if id==4 {24}else{0},
        "isSelf":id==0,"isObserver":id==4,"teamChangeEnabled":true})).collect::<Vec<_>>() }),
    )
    .unwrap()
}
fn chat(sender: &str, content: &str) -> Value {
    json!({"messageType":"ChatMessage","payload":{"message":{"source":"gameChat","sender":sender,"content":content}}})
}
#[test]
fn majority_excludes_self_observers_and_ai_and_counts_each_player_once() {
    let mut commands = LobbyCommands::default();
    commands.observe(Some(&lobby()), 0);
    let settings = CommandSettings::default();
    for sender in ["Player0", "Player4", "Player5", "Unknown"] {
        assert!(
            commands
                .handle(&chat(sender, "?votestart"), &settings, 1)
                .reply
                .is_none()
        );
    }
    assert!(
        !commands
            .handle(&chat("Player1", "?votestart"), &settings, 1)
            .start
    );
    assert!(
        !commands
            .handle(&chat("Player1", "?votestart"), &settings, 2)
            .start
    );
    assert!(
        commands
            .handle(&chat("Player2", "?votestart"), &settings, 2)
            .start
    );
}
#[test]
fn disabled_commands_external_channels_and_spoofed_text_are_ignored() {
    let mut commands = LobbyCommands::default();
    commands.observe(Some(&lobby()), 0);
    let mut settings = CommandSettings::default();
    settings.set(ChatCommand::VoteStart, false);
    assert!(
        commands
            .handle(&chat("Player1", "?votestart"), &settings, 1)
            .reply
            .is_none()
    );
    let mut whisper = chat("Player1", "?help");
    whisper["payload"]["message"]["source"] = json!("whisper");
    assert!(commands.handle(&whisper, &settings, 2).reply.is_none());
    assert!(
        commands
            .handle(&chat("Player1", "Player2: ?help"), &settings, 3)
            .reply
            .is_none()
    );
    assert!(
        commands
            .handle(&chat("Player1", "!help"), &settings, 4)
            .reply
            .is_none()
    );
    assert!(
        !commands
            .handle(&chat("Player1", "?help"), &settings, 5)
            .reply
            .unwrap()
            .payload["content"]
            .as_str()
            .unwrap()
            .contains("?votestart")
    );
}
#[test]
fn votes_expire_and_cannot_pass_on_departure_or_cross_lobbies() {
    let mut commands = LobbyCommands::default();
    let settings = CommandSettings::default();
    let mut roster = lobby();
    commands.observe(Some(&roster), 0);
    commands.handle(&chat("Player1", "?votestart"), &settings, 1);
    commands.observe(Some(&roster), 61);
    assert!(
        !commands
            .handle(&chat("Player2", "?votestart"), &settings, 61)
            .start
    );
    roster.players.retain(|p| p.name != "Player3");
    commands.observe(Some(&roster), 62);
    assert!(commands.votes.is_empty());
    commands.handle(&chat("Player1", "?votestart"), &settings, 63);
    roster.lobby_name = "Test #2".into();
    commands.observe(Some(&roster), 64);
    assert!(commands.votes.is_empty());
    commands.observe(None, 65);
    assert!(commands.roster.is_empty());
}
#[test]
fn withdrawing_a_start_vote_does_not_start_a_game() {
    let mut commands = LobbyCommands::default();
    let settings = CommandSettings::default();
    commands.observe(Some(&lobby()), 0);
    commands.handle(&chat("Player1", "?votestart"), &settings, 1);
    assert!(
        !commands
            .handle(&chat("Player1", "?votecancel"), &settings, 2)
            .start
    );
    assert!(commands.votes.is_empty());
}

#[test]
fn kick_needs_distinct_votes_and_a_fresh_matching_target() {
    let mut commands = LobbyCommands::default();
    let settings = CommandSettings::default();
    let mut roster = lobby();
    commands.observe(Some(&roster), 0);
    for now in [1, 2] {
        assert!(
            commands
                .handle(&chat("Player1", "?votekick Player3"), &settings, now)
                .kick
                .is_none()
        );
    }
    let target = commands
        .handle(&chat("Player2", "?votekick Player3"), &settings, 3)
        .kick
        .unwrap();
    let kick = target.confirm(&roster).unwrap();
    assert_eq!(kick.message, "KickPlayerFromGameLobby");
    assert_eq!(kick.payload["slot"], 3);
    roster.players[3].name = "Replacement".into();
    assert!(target.confirm(&roster).is_none());
    roster = lobby();
    roster.players[3].id = Some(99);
    assert!(target.confirm(&roster).is_none());
    roster = lobby();
    roster.lobby_name = "Test #2".into();
    assert!(target.confirm(&roster).is_none());
}

#[test]
fn kick_rejects_self_observers_ai_small_rosters_and_ambiguous_names() {
    let settings = CommandSettings::default();
    for target in ["Player1", "Player0", "Player4", "Player5"] {
        let mut commands = LobbyCommands::default();
        commands.observe(Some(&lobby()), 0);
        commands.handle(
            &chat("Player1", &format!("?votekick {target}")),
            &settings,
            1,
        );
        assert!(commands.kick.is_none());
    }
    let mut roster = lobby();
    roster.players[2].name = "Twin#1".into();
    roster.players[3].name = "Twin#2".into();
    let mut commands = LobbyCommands::default();
    commands.observe(Some(&roster), 0);
    commands.handle(&chat("Player1", "?votekick Twin"), &settings, 1);
    assert!(commands.kick.is_none());
    commands.handle(&chat("Player1", "?votekick Twin#1"), &settings, 2);
    assert!(commands.kick.is_some());
    roster.players.remove(3);
    commands.observe(Some(&roster), 3);
    commands.handle(&chat("Player1", "?votekick Twin#1"), &settings, 3);
    assert!(commands.kick.is_none());
}

#[test]
fn a_pending_kick_is_cancelled_if_a_voter_leaves_or_changes_identity() {
    let mut commands = LobbyCommands::default();
    let settings = CommandSettings::default();
    commands.observe(Some(&lobby()), 0);
    commands.handle(&chat("Player1", "?votekick Player3"), &settings, 1);
    let target = commands
        .handle(&chat("Player2", "?votekick Player3"), &settings, 2)
        .kick
        .unwrap();
    for change in 0..4 {
        let mut roster = lobby();
        match change {
            0 => roster.players[1].slot_status = 0,
            1 => roster.players[1].is_observer = true,
            2 => roster.players[1].id = Some(80),
            _ => {
                roster.players[4].is_observer = false;
                roster.players[4].team = 0;
            }
        }
        assert!(target.confirm(&roster).is_none());
    }
    let mut roster = lobby();
    roster.players[4].is_observer = false;
    roster.players[4].team = 0;
    roster.players[4].name.clear();
    assert!(
        target.confirm(&roster).is_none(),
        "An unnamed join invalidates the pending vote"
    );
    let mut roster = lobby();
    roster.players[0].name = "Ghost".into();
    assert!(
        target.confirm(&roster).is_some(),
        "A host alias refresh does not change the electorate"
    );
}

#[test]
fn live_disabling_clears_votes_and_old_ready_settings_migrate() {
    let mut settings: CommandSettings =
        serde_json::from_str(r#"["ready","unready","help","vote_start"]"#).unwrap();
    assert!(settings.enabled(ChatCommand::VoteKick));
    assert!(!serde_json::to_string(&settings).unwrap().contains("ready"));
    let mut commands = LobbyCommands::default();
    commands.observe(Some(&lobby()), 0);
    commands.handle(&chat("Player1", "?votestart"), &settings, 1);
    commands.handle(&chat("Player1", "?votekick Player3"), &settings, 2);
    settings.set(ChatCommand::VoteStart, false);
    settings.set(ChatCommand::VoteKick, false);
    commands.apply_settings(&settings);
    assert!(commands.kick.is_none() && commands.votes.is_empty());
    settings.set(ChatCommand::VoteKick, true);
    assert!(
        commands
            .handle(&chat("Player2", "?votekick Player3"), &settings, 3)
            .kick
            .is_none()
    );
    commands.observe(Some(&lobby()), 64);
    assert!(commands.kick.is_none());
    let mut unknown = lobby();
    unknown.players[3].name.clear();
    commands.observe(Some(&unknown), 65);
    assert!(
        commands
            .handle(
                &chat("Player1", "?votestart"),
                &CommandSettings::default(),
                66
            )
            .reply
            .is_none()
    );
}
