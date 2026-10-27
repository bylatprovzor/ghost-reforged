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
    fetch_for(tags, false)
}

pub(super) fn fetch_for(tags: &[String; 4], mmr: bool) -> Result<[Profile; 4], Error> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .new_agent();
    let mut request = agent.get("https://api.w3ladder.net/v1/profiles");
    if mmr {
        request = request.query("rating", "mmr");
    }
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
    validate_for(&body, tags, mmr)
}

pub(super) fn validate(body: &str, tags: &[String; 4]) -> Result<[Profile; 4], Error> {
    let data: Snapshot = serde_json::from_str(body).map_err(|_| Error::InvalidRoster)?;
    validate_for(body, tags, data.version == 2)
}

fn validate_for(body: &str, tags: &[String; 4], mmr: bool) -> Result<[Profile; 4], Error> {
    let data: Snapshot = serde_json::from_str(body).map_err(|_| Error::InvalidRoster)?;
    if data.version != (if mmr { 2 } else { 1 })
        || (if mmr { ![super::MINIMUM_MMR_GAMES, 10].contains(&data.minimum_games) } else { data.minimum_games != 3 })
        || data.players.len() != 4
    {
        return Err(Error::InvalidRoster);
    }
    for (i, p) in data.players.iter().enumerate() {
        if p.slot as usize != i
            || !p.battletag.eq_ignore_ascii_case(&tags[i])
            || !p.valid_rank()
            || (mmr && (p.mmr.is_none() || p.mmr_games.is_none() ||
                (p.rank.is_some() && p.mmr_games.is_some_and(|n| n < data.minimum_games))))
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
    fn preview_must_match_the_requested_rating_version() {
        let tags = ["A#1234", "B#1234", "C#1234", "D#1234"].map(String::from);
        let mut data = json!({"version":1,"minimum_games":3,"players":
            tags.iter().enumerate().map(|(slot, tag)| json!({"slot":slot,
                "battletag":tag,"wins":2,"losses":1,"rank":null,"mmr":1000,"mmr_games":0})).collect::<Vec<_>>()});
        assert!(validate_for(&data.to_string(), &tags, true).is_err());
        data["version"] = json!(2);
        data["minimum_games"] = json!(3);
        assert!(validate_for(&data.to_string(), &tags, true).is_ok());
        assert!(validate_for(&data.to_string(), &tags, false).is_err());
    }

    #[test]
    fn third_validated_game_qualifies_mmr_rank_in_profiles_and_lobby() {
        let tags = ["A#1234", "B#1234", "C#1234", "D#1234"].map(String::from);
        let mut data = json!({"version":2,"minimum_games":3,"players":
            tags.iter().enumerate().map(|(slot, tag)| json!({"slot":slot,
                "battletag":tag,"wins":1,"losses":2,"rank":1,"mmr":1000,"mmr_games":3})).collect::<Vec<_>>()});
        let profiles = validate_for(&data.to_string(), &tags, true).unwrap();
        assert_eq!(profiles[0].mmr_summary(), "MMR: 1000, Rank: #1, Wins: 1, Loss: 2");
        data["players"][0]["mmr_games"] = json!(2);
        assert!(validate_for(&data.to_string(), &tags, true).is_err());
        data["players"][0]["rank"] = serde_json::Value::Null;
        let profiles = validate_for(&data.to_string(), &tags, true).unwrap();
        assert_eq!(profiles[0].mmr_summary(), "MMR: 1000 (provisional), Rank: ???, Wins: 1, Loss: 2");
        data["minimum_games"] = json!(10);
        assert!(validate_for(&data.to_string(), &tags, true).is_err());
        for p in data["players"].as_array_mut().unwrap() { p["rank"] = serde_json::Value::Null; }
        let old = validate_for(&data.to_string(), &tags, true).unwrap();
        assert_eq!(old[1].mmr_summary(), "MMR: 1000 (provisional), Rank: ???, Wins: 1, Loss: 2");
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
