use super::{Error, Packet, Profile};
use serde_json::{Value, json};
use std::time::Duration;

pub struct Prepared {
    pub profiles: [Profile; 4],
    pub packet: Packet,
    pub expires_at: Option<u64>,
}

impl Prepared {
    pub fn preview(profiles: [Profile; 4]) -> Result<Self, Error> {
        let packet = Packet::encode(&profiles)?;
        Ok(Self {
            profiles,
            packet,
            expires_at: None,
        })
    }
}

// Only the host machine needs this credential. Never write it into the map,
// lobby, application logs, or a serialized user configuration.
pub fn prepare(tags: &[String; 4]) -> Result<Prepared, Error> {
    let mode = std::env::var("W3LADDER_MODE").unwrap_or_else(|_| "ranked".into());
    if mode == "preview" {
        return Prepared::preview(super::fetch(tags)?);
    }
    let Some(key) = std::env::var_os("W3LADDER_COLLECTOR_KEY") else {
        return Err(Error::Unavailable);
    };
    let key = key.to_str().ok_or(Error::Unavailable)?;
    if !(43..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::Unavailable);
    }
    if !matches!(mode.as_str(), "ranked" | "test") {
        return Err(Error::Unavailable);
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| Error::Unavailable)?;
    let match_id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let payload = json!({"match_id":match_id,"map_name":super::MAP_NAME,
        "map_version":"1.59","mode":mode,"players":tags.iter().enumerate()
            .map(|(slot,tag)| json!({"slot":slot,"battletag":tag})).collect::<Vec<_>>()});
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .new_agent();
    let mut response = agent
        .post("https://api.w3ladder.net/v1/ladder/sessions")
        .header("Authorization", &format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send(payload.to_string())
        .map_err(|_| Error::Unavailable)?;
    let body = response
        .body_mut()
        .with_config()
        .limit(16 * 1024)
        .read_to_string()
        .map_err(|_| Error::Unavailable)?;
    validate(&body, tags, &match_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Registers an isolated test session against the live ladder API"]
    fn live_session_registration_returns_an_encodable_result_ticket() {
        assert_eq!(std::env::var("W3LADDER_MODE").as_deref(), Ok("test"));
        let tags = std::array::from_fn(|i| format!("LadderRelayTest{i}#900001"));
        let prepared = prepare(&tags).unwrap();
        assert_eq!(prepared.packet.handicaps[1], 84);
        assert!(prepared.expires_at.is_some());
        assert!(
            prepared
                .profiles
                .iter()
                .all(|p| p.wins == 0 && p.losses == 0)
        );
    }

    #[test]
    fn session_reply_must_match_the_request_and_have_a_live_complete_ticket() {
        let tags = std::array::from_fn(|i| format!("Player{i}#1234"));
        let mut data = json!({"version":2,"minimum_games":3,"match_id":"test-session-001",
            "ticket":"0123456789abcde","expires_at":4102444800u64,
            "players":tags.iter().enumerate().map(|(slot,tag)|
                json!({"slot":slot,"battletag":tag,"wins":0,"losses":0,"rank":null}))
                .collect::<Vec<_>>()});
        assert!(validate(&data.to_string(), &tags, "test-session-001").is_ok());
        assert!(validate(&data.to_string(), &tags, "different-session").is_err());
        data["ticket"] = json!("ffffffffffffffff");
        assert!(validate(&data.to_string(), &tags, "test-session-001").is_err());
        data["ticket"] = json!("0123456789abcde");
        data["expires_at"] = json!(0);
        assert!(validate(&data.to_string(), &tags, "test-session-001").is_err());
    }
}

fn validate(body: &str, tags: &[String; 4], match_id: &str) -> Result<Prepared, Error> {
    let mut data: Value = serde_json::from_str(body).map_err(|_| Error::InvalidRoster)?;
    if data["version"] != 2 || data["match_id"] != match_id {
        return Err(Error::InvalidRoster);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Unavailable)?
        .as_secs();
    if data["expires_at"]
        .as_u64()
        .is_none_or(|expiry| expiry < now + 3600)
    {
        return Err(Error::InvalidRoster);
    }
    let ticket = data["ticket"]
        .as_str()
        .ok_or(Error::InvalidRoster)?
        .to_owned();
    data["version"] = json!(1);
    let profiles = super::api::validate(&data.to_string(), tags)?;
    let packet = Packet::encode_session(&profiles, &ticket)?;
    Ok(Prepared {
        profiles,
        packet,
        expires_at: data["expires_at"].as_u64(),
    })
}
