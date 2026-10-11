use super::{Event, status};
use crate::{
    alias::Alias,
    background,
    config::Config,
    discovery::{self, DiscoveryError},
    protocol::Lobby,
    transport::Client,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct Connection {
    pub initialization_required: bool,
    pub client: Client,
    pub alias: Alias,
    pub game: background::Background,
    pub pending: VecDeque<Value>,
}

pub(super) fn open(
    config: &Config,
    name: String,
    tx: &Sender<Event>,
    stop: &AtomicBool,
    repair: &mut super::repair::Repair,
) -> Result<Option<Connection>, String> {
    loop {
        let Some(mut connection) = open_once(config, name.clone(), tx, stop)? else {
            return Ok(None);
        };
        let candidate = connection
            .pending
            .iter()
            .rev()
            .find(|event| event["messageType"] == "GameLobbySetup")
            .and_then(|event| serde_json::from_value::<Lobby>(event["payload"].clone()).ok())
            .filter(crate::recovery::incomplete_lobby);
        if let Some(candidate) = candidate
            && repair.confirm(
                &candidate,
                &mut connection.client,
                &mut connection.pending,
                tx,
                stop,
            )?
        {
            if stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            status(tx, "Incomplete lobby detected. Restarting Warcraft III…");
            connection.game.terminate()?;
            continue;
        }
        return Ok(Some(connection));
    }
}

fn open_once(
    config: &Config,
    name: String,
    tx: &Sender<Event>,
    stop: &AtomicBool,
) -> Result<Option<Connection>, String> {
    status(tx, "Connecting to Warcraft III…");
    let mut background = None;
    let mut launcher = background::LauncherLaunch::default();
    let discovery = discovery::discover();
    let initialization_required = needs_initialization(&discovery);
    let mut launch_requested = false;
    let endpoint = match discovery {
        Ok(endpoint) => endpoint,
        Err(DiscoveryError::NotRunning | DiscoveryError::NoEndpoint) => {
            if matches!(discovery, Err(DiscoveryError::NotRunning)) {
                status(tx, "Starting Warcraft III through Battle.net…");
                background = background::launch(&mut launcher)?;
                launch_requested = true;
            }
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                launcher.poll()?;
                if background.is_none()
                    && let Ok(pid) = discovery::process_id()
                {
                    background = Some(background::Background::attach(pid)?);
                }
                if let Some(game) = background.as_mut() {
                    game.hide()?;
                }
                if stop.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                let discovered = discovery::discover();
                if request_launch(&discovered, &mut launch_requested) {
                    // An endpoint-less client can exit between discovery attempts.
                    drop(background.take());
                    status(tx, "Starting Warcraft III through Battle.net…");
                    background = background::launch(&mut launcher)?;
                }
                match discovered {
                    Ok(endpoint)
                        if background
                            .as_ref()
                            .is_none_or(|game| endpoint.pid == game.pid()) =>
                    {
                        break endpoint;
                    }
                    Ok(_) | Err(DiscoveryError::Ambiguous) => {
                        return Err("Another Warcraft III client appeared.".into());
                    }
                    Err(DiscoveryError::NotRunning | DiscoveryError::NoEndpoint) => {}
                    Err(error) => return Err(error.to_string()),
                }
                if Instant::now() >= deadline {
                    return Err("Warcraft III did not become available. Open it through Battle.net, then click Host.".into());
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        Err(e) => return Err(e.to_string()),
    };
    if endpoint.version.as_deref() != Some(config.expected_version.as_str()) {
        return Err("This Warcraft III build has not been verified yet.".into());
    }
    let mut game = match background {
        Some(game) => game,
        None => background::Background::attach(endpoint.pid)?,
    };
    game.hide()?;
    let mut client = Client::connect(&endpoint).map_err(|e| e.to_string())?;
    let alias = Alias::open(endpoint.pid, name)?;
    client
        .send("SendGameLobbySetup", json!({}))
        .map_err(|e| e.to_string())?;
    client
        .send("SendGameListing", json!({}))
        .map_err(|e| e.to_string())?;
    client
        .send("GetGameList", json!({}))
        .map_err(|e| e.to_string())?;
    let mut pending = VecDeque::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if let Some(event) = client.receive().map_err(|e| e.to_string())? {
            pending.push_back(event);
        }
    }
    let screen = pending
        .iter()
        .rev()
        .find(|event| event["messageType"] == "SetGlueScreen")
        .and_then(|event| event["payload"]["screen"].as_str());
    if matches!(screen, Some("SCORE_SCREEN" | "LOADING_SCREEN")) {
        pending.retain(|event| event["messageType"] == "SetGlueScreen");
    } else {
        for event in &pending {
            if event["messageType"] == "GameLobbySetup" {
                let value: Lobby = serde_json::from_value(event["payload"].clone())
                    .map_err(|_| "The native lobby format changed.")?;
                if !crate::recovery::incomplete_lobby(&value)
                    && (value.local().is_some() || value.is_host)
                    && crate::recovery::resumable_lobby(config, &value).is_none()
                {
                    return Err("This lobby belongs to another session. Leave it before hosting with this profile.".into());
                }
            }
        }
        // A resumable roster must reach the engine before any listing can create another lobby.
        prioritize_adoption(&mut pending);
    }
    if stop.load(Ordering::SeqCst) {
        return Ok(None);
    }
    launcher.complete();
    Ok(Some(Connection {
        initialization_required,
        client,
        alias,
        game,
        pending,
    }))
}

fn prioritize_adoption(pending: &mut VecDeque<Value>) {
    if let Some(index) = pending
        .iter()
        .position(|event| event["messageType"] == "GameLobbySetup")
    {
        // Repair must see the chronological menu/placeholder sequence. Only a
        // populated roster can authorize adoption ahead of an older listing.
        if serde_json::from_value::<Lobby>(pending[index]["payload"].clone())
            .is_ok_and(|lobby| crate::recovery::incomplete_lobby(&lobby))
        {
            return;
        }
        let first = pending
            .remove(index)
            .expect("index came from the same queue");
        pending.push_front(first);
    }
}

fn needs_initialization(discovery: &Result<discovery::Endpoint, DiscoveryError>) -> bool {
    matches!(
        discovery,
        Err(DiscoveryError::NotRunning | DiscoveryError::NoEndpoint)
    )
}

fn request_launch(
    discovered: &Result<discovery::Endpoint, DiscoveryError>,
    requested: &mut bool,
) -> bool {
    if !*requested && matches!(discovered, Err(DiscoveryError::NotRunning)) {
        *requested = true;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_keeps_menu_and_placeholder_in_chronological_order_for_repair() {
        let mut pending = VecDeque::from([
            json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}}),
            json!({"messageType":"GameLobbySetup","payload":{
                "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
                "players":[{"slot":0,"slotStatus":0,"slotType":0,"team":255,
                    "isSelf":false,"isObserver":false,"teamChangeEnabled":false}]
            }}),
        ]);
        let original = pending.clone();
        prioritize_adoption(&mut pending);
        assert_eq!(pending, original);
    }

    #[test]
    fn a_client_that_exits_before_exposing_its_endpoint_gets_one_launch_request() {
        let mut requested = false;
        assert!(!request_launch(
            &Err(DiscoveryError::NoEndpoint),
            &mut requested
        ));
        assert!(request_launch(
            &Err(DiscoveryError::NotRunning),
            &mut requested
        ));
        assert!(!request_launch(
            &Err(DiscoveryError::NotRunning),
            &mut requested
        ));
        assert!(!request_launch(
            &Err(DiscoveryError::NoEndpoint),
            &mut requested
        ));
    }

    #[test]
    fn adoption_preserves_later_countdown_and_roster_order() {
        let mut pending: VecDeque<Value> = [
            json!({"messageType":"GameList"}),
            json!({"messageType":"GameLobbySetup","roster":"A"}),
            json!({"messageType":"LobbyCountdown","active":true}),
            json!({"messageType":"LobbyCountdown","active":false}),
            json!({"messageType":"GameLobbySetup","roster":"B"}),
        ]
        .into();
        let expected = [
            pending[1].clone(),
            pending[0].clone(),
            pending[2].clone(),
            pending[3].clone(),
            pending[4].clone(),
        ];
        prioritize_adoption(&mut pending);
        assert_eq!(pending, VecDeque::from(expected));
    }

    #[test]
    fn both_startup_paths_wait_for_native_initialization() {
        assert!(needs_initialization(&Err(DiscoveryError::NotRunning)));
        assert!(needs_initialization(&Err(DiscoveryError::NoEndpoint)));
        assert!(!needs_initialization(&Ok(discovery::Endpoint {
            pid: 1,
            ports: Vec::new(),
            session: String::new(),
            version: None,
        })));
    }
}
