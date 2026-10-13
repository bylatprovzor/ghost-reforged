use super::{Error, Profile};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct Snapshot {
    version: u32,
    minimum_games: u32,
    players: Vec<Profile>,
}

// Called on a worker thread; neither TLS nor server delays block lobby control.
pub fn fetch(tags: &[String; 4]) -> Result<[Profile; 4], Error> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .new_agent();
    let mut request = agent.get("https://api.w3ladder.net/v1/profiles");
    for tag in tags {
        request = request.query("battletag", tag);
    }
    let mut response = request.call().map_err(|_| Error::Unavailable)?;
    let body = response
        .body_mut()
        .with_config()
        .limit(16 * 1024)
        .read_to_string()
        .map_err(|_| Error::Unavailable)?;
    validate(&body, tags)
}

fn validate(body: &str, tags: &[String; 4]) -> Result<[Profile; 4], Error> {
    let data: Snapshot = serde_json::from_str(body).map_err(|_| Error::InvalidRoster)?;
    if data.version != 1 || data.minimum_games != 3 || data.players.len() != 4 {
        return Err(Error::InvalidRoster);
    }
    for (i, p) in data.players.iter().enumerate() {
        let games = u64::from(p.wins) + u64::from(p.losses);
        if p.slot as usize != i
            || !p.battletag.eq_ignore_ascii_case(&tags[i])
            || p.rank == Some(0)
            || (games < 3 && p.rank.is_some())
        {
            return Err(Error::InvalidRoster);
        }
    }
    data.players.try_into().map_err(|_| Error::InvalidRoster)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "Read-only check against the deployed profile API"]
    fn live_profile_api_returns_an_encodable_snapshot() {
        let tags = [
            "ArteMy#21249",
            "Shideneyu#1234",
            "EmberClaw#4821",
            "MoonWarden#2058",
        ]
        .map(String::from);
        let profiles = fetch(&tags).expect("Live profile request");
        super::super::Packet::encode(&profiles).expect("Snapshot fits the lobby packet");
    }

    #[test]
    fn rejects_reordered_stale_and_ineligible_profiles() {
        let tags = ["A#1234", "B#1234", "C#1234", "D#1234"].map(String::from);
        let mut data = json!({"version":1,"minimum_games":3,"players":
            tags.iter().enumerate().map(|(slot, tag)| json!({"slot":slot,
                "battletag":tag,"wins":2,"losses":1,"rank":1})).collect::<Vec<_>>()});
        assert!(validate(&data.to_string(), &tags).is_ok());
        data["players"][0]["losses"] = json!(0);
        assert!(validate(&data.to_string(), &tags).is_err());
        data["players"][0]["rank"] = json!(null);
        assert!(validate(&data.to_string(), &tags).is_ok());
        data["players"][0]["battletag"] = json!("Different#1234");
        assert!(validate(&data.to_string(), &tags).is_err());
    }
}
