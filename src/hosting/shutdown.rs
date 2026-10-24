use crate::{
    engine::Engine,
    protocol::{self, Command, Lobby},
    transport::Client,
};
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const UNCONFIRMED: &str =
    "Hosting stopped, but lobby closure could not be confirmed. Inspect Warcraft III.";

/// Cleanup failures must neither skip native departure nor turn into a successful Stop.
pub(super) fn finish(
    resumed: Result<(), String>,
    identity: Result<(), String>,
    close: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let errors: Vec<_> = [resumed, identity, close()]
        .into_iter()
        .filter_map(Result::err)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join(" "))
    }
}

pub(super) fn stop(
    resources: &mut super::supervision::Resources,
    engine: &Engine,
    client: &mut Client,
    pending: &mut VecDeque<super::pending::Callback>,
) -> Result<(), String> {
    let resumed = resources
        .game
        .as_mut()
        .map_or(Ok(()), |game| game.low_resource(false));
    let identity = resources
        .identity
        .as_mut()
        .map_or(Ok(()), |identity| identity.cancel_creation());
    finish(resumed, identity, || {
        close(engine, client, pending, &resources.progress)
    })
}

/// Resolve the current native state before leaving, even if the last roster expired.
pub(super) fn close(
    engine: &Engine,
    client: &mut Client,
    pending: &mut VecDeque<super::pending::Callback>,
    progress: &super::health::Progress,
) -> Result<(), String> {
    let Some(expected) = engine.shutdown_identity() else {
        return Ok(());
    };
    let mut shutdown = Shutdown {
        expected,
        sent: false,
    };
    // Old rosters cannot authorize departure. Screen/leave events can prove the lobby is gone.
    let mut outside = false;
    while let Some(event) = pending.pop_front() {
        observe_location(&event, &mut outside);
    }
    let drain_until = Instant::now() + Duration::from_millis(50);
    while Instant::now() < drain_until {
        let Some(event) = client.receive_available().map_err(|_| UNCONFIRMED)? else {
            break;
        };
        observe_location(&event, &mut outside);
    }
    if outside {
        return Ok(());
    }
    client
        .send("SendGameLobbySetup", serde_json::json!({}))
        .map_err(|_| UNCONFIRMED)?;
    let deadline = Instant::now() + Duration::from_secs(engine.config.command_timeout_seconds);
    while Instant::now() < deadline {
        // This is the worker's bounded native wait, not a frontend liveness pulse.
        progress.pulse(super::health::Stage::Observing);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Some(event) = client
            .receive_with_timeout(remaining.min(Duration::from_millis(250)))
            .map_err(|_| shutdown.unconfirmed("Native connection failed"))?
        {
            let Some(commands) = shutdown.observe(&event)? else {
                return Ok(());
            };
            for command in commands {
                client
                    .send(command.message, command.payload)
                    .map_err(|_| shutdown.unconfirmed("Native departure command failed"))?;
            }
        }
    }
    Err(shutdown.unconfirmed("Command deadline expired"))
}

fn departed(event: &Value) -> bool {
    event["messageType"] == "MultiplayerGameLeave"
        || (event["messageType"] == "SetGlueScreen"
            && matches!(
                event["payload"]["screen"].as_str(),
                Some(
                    "LOADING_SCREEN"
                        | "SCORE_SCREEN"
                        | "MAIN_MENU"
                        | "MULTIPLAYER_CUSTOM_GAMES_SETUP"
                        | "CUSTOM_LOBBIES"
                )
            ))
}

fn observe_location(event: &Value, outside: &mut bool) {
    if departed(event) {
        *outside = true;
    } else if event["messageType"] == "GameLobbySetup"
        || (event["messageType"] == "SetGlueScreen" && event["payload"]["screen"] == "GAME_LOBBY")
    {
        *outside = false;
    }
}

struct Shutdown {
    expected: (String, String),
    sent: bool,
}

impl Shutdown {
    fn unconfirmed(&self, reason: &str) -> String {
        let awaiting = if self.sent {
            "departure acknowledgement"
        } else {
            "fresh lobby roster"
        };
        format!("{UNCONFIRMED} {reason} while awaiting {awaiting}.")
    }

    fn observe(&mut self, event: &Value) -> Result<Option<Vec<Command>>, String> {
        if departed(event) {
            return Ok(None);
        }
        if event["messageType"] != "GameLobbySetup" {
            return Ok(Some(Vec::new()));
        }
        let lobby: Lobby =
            serde_json::from_value(event["payload"].clone()).map_err(|_| UNCONFIRMED)?;
        if lobby.local().is_none() {
            return Ok(None);
        }
        if !lobby.is_host {
            return Err("Hosting stopped; host authority changed before departure was confirmed. Inspect Warcraft III.".into());
        }
        if lobby.lobby_name != self.expected.0
            || !protocol::same_map_path(&lobby.map_data.map_path, &self.expected.1)
        {
            return Err("Hosting stopped; the native lobby changed. Inspect Warcraft III.".into());
        }
        if self.sent {
            return Ok(Some(Vec::new()));
        }
        self.sent = true;
        Ok(Some(vec![
            Command::empty("LobbyCancel"),
            Command::empty("LeaveGame"),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn failed_render_resume_still_closes_and_preserves_every_cleanup_error() {
        let mut closed = false;
        let result = finish(
            Err("Resume unconfirmed.".into()),
            Err("Identity failed.".into()),
            || {
                closed = true;
                Err("Lobby departure unconfirmed.".into())
            },
        );
        assert!(closed);
        assert_eq!(
            result,
            Err("Resume unconfirmed. Identity failed. Lobby departure unconfirmed.".into())
        );
        assert!(finish(Err("Resume unconfirmed.".into()), Ok(()), || Ok(())).is_err());
        assert!(finish(Ok(()), Ok(()), || Ok(())).is_ok());
    }
    fn roster(name: &str) -> Value {
        json!({"messageType":"GameLobbySetup","payload":{"isHost":true,"lobbyName":name,"mapData":{"mapPath":"map.w3x"},
            "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":24,"isSelf":true,"isObserver":true,"teamChangeEnabled":true}]}})
    }

    #[test]
    fn stale_roster_shutdown_refreshes_cancels_leaves_and_waits_for_ack() {
        shutdown_roundtrip(None, Duration::ZERO, Duration::ZERO, 30).unwrap();
    }

    #[test]
    fn stop_during_recovery_backoff_still_closes_the_retained_owned_lobby() {
        shutdown_roundtrip(
            Some(crate::engine::Phase::Recovering(100)),
            Duration::ZERO,
            Duration::ZERO,
            30,
        )
        .unwrap();
    }

    #[test]
    fn stop_before_empty_refresh_leave_is_sent_still_departs() {
        shutdown_roundtrip(
            Some(crate::engine::Phase::Leaving(100)),
            Duration::ZERO,
            Duration::ZERO,
            30,
        )
        .unwrap();
    }

    #[test]
    fn stop_accepts_delayed_roster_within_the_configured_command_budget() {
        shutdown_roundtrip(None, Duration::from_secs(6), Duration::ZERO, 10).unwrap();
    }

    #[test]
    fn recovery_stop_accepts_delayed_departure_within_the_configured_command_budget() {
        shutdown_roundtrip(
            Some(crate::engine::Phase::Recovering(100)),
            Duration::ZERO,
            Duration::from_secs(6),
            10,
        )
        .unwrap();
    }

    #[test]
    fn stop_still_rejects_departure_after_the_configured_deadline() {
        let error =
            shutdown_roundtrip(None, Duration::ZERO, Duration::from_secs(6), 5).unwrap_err();
        assert!(
            error.contains("Command deadline expired while awaiting departure acknowledgement")
        );
    }

    #[test]
    fn stop_reports_missing_fresh_roster_separately_from_departure() {
        let error =
            shutdown_roundtrip(None, Duration::from_secs(6), Duration::ZERO, 5).unwrap_err();
        assert!(error.contains("Command deadline expired while awaiting fresh lobby roster"));
    }

    fn shutdown_roundtrip(
        interrupted: Option<crate::engine::Phase>,
        roster_delay: Duration,
        departure_delay: Duration,
        command_timeout_seconds: u64,
    ) -> Result<(), String> {
        use crate::{config::Config, discovery::Endpoint};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(12)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            for expected in ["SendGameLobbySetup", "LobbyCancel", "LeaveGame"] {
                let Ok(frame) = socket.read() else { return };
                let command: Value = serde_json::from_str(&frame.into_text().unwrap()).unwrap();
                assert_eq!(command["message"], expected);
                if expected == "SendGameLobbySetup" {
                    std::thread::sleep(roster_delay);
                    let _ = socket.send(tungstenite::Message::Text(
                        roster("Test #1").to_string().into(),
                    ));
                }
            }
            std::thread::sleep(departure_delay);
            let _ = socket.send(tungstenite::Message::Text(
                json!({"messageType":"MultiplayerGameLeave"})
                    .to_string()
                    .into(),
            ));
        });
        let mut engine = Engine::new(Config {
            maps: vec!["map.w3x".into()],
            lobby_name: "Test #{n}".into(),
            first_number: 2,
            resume_lobby: true,
            command_timeout_seconds,
            ..Config::default()
        });
        engine.on_event(&roster("Test #1"), 0);
        assert!(engine.owned_lobby(100).is_none());
        let mut client = Client::connect(&Endpoint {
            pid: 0,
            ports: vec![port],
            session: "test-session".into(),
            version: None,
        })
        .unwrap();
        // A previous leave event must not hide the more recent owned roster.
        let mut pending = VecDeque::from([
            json!({"messageType":"MultiplayerGameLeave"}).into(),
            roster("Test #1").into(),
        ]);
        let result = if let Some(phase) = interrupted {
            let mut resources = super::super::supervision::Resources::default();
            engine.phase = phase;
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(super::super::Control::Stop).unwrap();
            assert!(!super::super::supervision::wait(
                Duration::from_secs(10),
                &rx,
                &std::sync::atomic::AtomicBool::new(false),
                &mut engine,
                100,
                &mut resources,
            ));
            stop(&mut resources, &engine, &mut client, &mut pending)
        } else {
            let progress = super::super::health::Progress::default();
            let result = close(&engine, &mut client, &mut pending, &progress);
            assert!(progress.snapshot().age_seconds < 2);
            result
        };
        drop(client);
        server.join().unwrap();
        result
    }
    #[test]
    fn departure_requires_native_confirmation_and_does_not_repeat_commands() {
        let mut shutdown = Shutdown {
            expected: ("Test #1".into(), "map.w3x".into()),
            sent: false,
        };
        let commands = shutdown.observe(&roster("Test #1")).unwrap().unwrap();
        assert_eq!(
            commands.iter().map(|c| c.message).collect::<Vec<_>>(),
            ["LobbyCancel", "LeaveGame"]
        );
        assert!(
            shutdown
                .observe(&roster("Test #1"))
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(
            shutdown
                .observe(&json!({"messageType":"MultiplayerGameLeave"}))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn changed_lobbies_are_preserved_and_loading_does_not_issue_game_commands() {
        let mut shutdown = Shutdown {
            expected: ("Test #1".into(), "map.w3x".into()),
            sent: false,
        };
        assert!(shutdown.observe(&roster("Other lobby")).is_err());
        assert!(!shutdown.sent);
        assert!(
            shutdown
                .observe(
                    &json!({"messageType":"SetGlueScreen","payload":{"screen":"LOADING_SCREEN"}})
                )
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn loss_of_host_authority_is_not_proof_of_departure() {
        for sent in [false, true] {
            let mut shutdown = Shutdown {
                expected: ("Test #1".into(), "map.w3x".into()),
                sent,
            };
            let mut changed = roster("Test #1");
            changed["payload"]["isHost"] = json!(false);
            assert!(shutdown.observe(&changed).is_err());
        }
    }
}
