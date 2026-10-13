use crate::protocol::Player;

// Wire values are shared with the map. Unknown is explicit, never guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Region {
    Unknown = 0,
    Europe = 1,
    Americas = 2,
    Asia = 3,
    China = 4,
}

impl Region {
    pub fn from_player(player: &Player) -> Self {
        match player.player_region.to_ascii_lowercase().as_str() {
            "eu" => Self::Europe,
            "us" | "use" | "usw" => Self::Americas,
            "kr" => Self::Asia,
            "cn" => Self::China,
            _ => match player.gateway_id {
                Some(10) => Self::Americas,
                Some(20) => Self::Europe,
                Some(30) => Self::Asia,
                _ => Self::Unknown,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_region_precedes_gateway_and_unknown_stays_unknown() {
        for (region, gateway, expected) in [
            ("EU", Some(10), Region::Europe),
            ("use", None, Region::Americas),
            ("usw", None, Region::Americas),
            ("kr", None, Region::Asia),
            ("cn", Some(30), Region::China),
            ("", Some(20), Region::Europe),
            ("", Some(10), Region::Americas),
            ("", Some(30), Region::Asia),
            ("", None, Region::Unknown),
            ("future", Some(99), Region::Unknown),
        ] {
            let player = serde_json::from_value(json!({
                "slot": 0, "slotStatus": 2, "slotType": 0, "team": 0,
                "isSelf": false, "isObserver": false, "teamChangeEnabled": true,
                "playerRegion": region, "gatewayId": gateway, "name": "Europe#1234"
            }))
            .unwrap();
            assert_eq!(Region::from_player(&player), expected);
        }
    }
}
