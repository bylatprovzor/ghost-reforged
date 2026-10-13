//! Pause only in a fresh, owned lobby; never during startup, loading or recovery.
use crate::engine::{Engine, Phase};

pub(super) fn pause_browser(engine: &Engine, identity_ready: bool, now: u64) -> bool {
    engine.config.headless
        && identity_ready
        && matches!(engine.phase, Phase::Lobby | Phase::Countdown(_))
        && engine.owned_lobby(now).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    fn owned() -> Engine {
        let mut engine = Engine::new(Config {
            headless: true,
            maps: vec!["map.w3x".into()],
            lobby_name: "Test #{n}".into(),
            resume_lobby: true,
            first_number: 2,
            ..Config::default()
        });
        engine.on_event(
            &json!({"messageType":"GameLobbySetup","payload":{
                "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
                "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":24,
                            "isSelf":true,"isObserver":true,"teamChangeEnabled":true}]
            }}),
            0,
        );
        engine
    }

    #[test]
    fn only_fresh_confirmed_owned_lobbies_pause_rendering() {
        let mut engine = owned();
        assert!(pause_browser(&engine, true, 0));
        assert!(!pause_browser(&engine, false, 0));
        assert!(!pause_browser(&engine, true, 1000));
        engine.phase = Phase::Countdown(10);
        assert!(pause_browser(&engine, true, 0));
        for phase in [
            Phase::AwaitMenu,
            Phase::Initializing(0),
            Phase::AwaitSignIn,
            Phase::Creating(0),
            Phase::Starting(0),
            Phase::InGame,
            Phase::WatchingGame,
            Phase::Restarting,
            Phase::Releasing(0),
            Phase::Leaving(0),
            Phase::Cooldown(0),
            Phase::Recovering(0),
            Phase::Stopped("test".into()),
        ] {
            engine.phase = phase;
            assert!(!pause_browser(&engine, true, 0), "{:?}", engine.phase);
        }
    }

    #[test]
    fn merely_changing_the_phase_cannot_pause_an_unowned_client() {
        let mut engine = Engine::new(Config::default());
        engine.phase = Phase::Lobby;
        assert!(!pause_browser(&engine, true, 0));
    }

    #[test]
    fn normal_mode_never_suspends_and_live_toggle_preserves_owned_lobby() {
        let mut engine = owned();
        let identity = engine.shutdown_identity();
        engine.config.headless = false;
        for phase in [Phase::Lobby, Phase::Countdown(10)] {
            engine.phase = phase;
            assert!(!pause_browser(&engine, true, 0));
            engine.config.headless = true;
            assert!(pause_browser(&engine, true, 0));
            engine.config.headless = false;
        }
        assert_eq!(engine.shutdown_identity(), identity);
    }
}
