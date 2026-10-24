//! Public ladder reads and the versioned lobby packet understood by the map.
mod api;
pub(crate) mod credential;
mod fields;
pub mod lookup;
mod mmr_packet;
mod packet;
mod region;
mod session;
pub use api::fetch;
pub use packet::{Packet, Setting};
pub use region::Region;
use serde::Deserialize;
pub use session::{Prepared, prepare, prepare_for_map};

pub const MINIMUM_MMR_GAMES: u32 = 3;

pub const MAP_NAME: &str = "Survival Chaos Reforged 1.59a Ranked";
pub const LEGACY_MAP_NAME: &str = "Survival Chaos Reforged Rank";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Profile {
    pub slot: u8,
    pub battletag: String,
    pub wins: u32,
    pub losses: u32,
    pub rank: Option<u32>,
    #[serde(default)]
    pub mmr: Option<i32>,
    #[serde(default)]
    pub mmr_games: Option<u32>,
}

impl Profile {
    pub fn mmr_summary(&self) -> String {
        let value = self.mmr.map_or_else(|| "???".into(), |m| m.to_string());
        let provisional = if self.mmr_games.is_some() && self.rank.is_none() {
            " (provisional)"
        } else {
            ""
        };
        format!(
            "MMR: {value}{provisional}, Rank: {}",
            self.rank.map_or_else(|| "???".into(), |r| format!("#{r}"))
        )
    }

    pub(super) fn valid_rank(&self) -> bool {
        self.rank != Some(0)
            && match (self.mmr, self.mmr_games) {
                (Some(_), Some(games)) => games >= MINIMUM_MMR_GAMES || self.rank.is_none(),
                (None, None) => {
                    u64::from(self.wins) + u64::from(self.losses) >= 3 || self.rank.is_none()
                }
                _ => false,
            }
    }
    pub fn announcement(&self) -> String {
        format!("{}: {}", self.battletag, self.summary())
    }

    pub fn summary(&self) -> String {
        format!(
            "Wins: {}, Loss: {}, Rank: {}",
            self.wins,
            self.losses,
            self.rank.map_or_else(|| "???".into(), |r| format!("#{r}"))
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Ladder service is unavailable")]
    Unavailable,
    #[error("Ladder response does not match the current four-player roster")]
    InvalidRoster,
    #[error("Ladder statistics exceed this map's packet capacity")]
    Capacity,
}

pub fn matches_map(name: &str) -> bool {
    let clean = clean_map_name(name);
    clean.eq_ignore_ascii_case(MAP_NAME) || clean.eq_ignore_ascii_case(LEGACY_MAP_NAME)
}

pub fn uses_mmr_packet(name: &str) -> bool {
    clean_map_name(name).eq_ignore_ascii_case(MAP_NAME)
}

fn clean_map_name(name: &str) -> String {
    let mut clean = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '|' && matches!(chars.peek(), Some('c' | 'C')) {
            chars.next();
            for _ in 0..8 {
                chars.next();
            }
        } else if c == '|' && matches!(chars.peek(), Some('r' | 'R')) {
            chars.next();
        } else {
            clean.push(c);
        }
    }
    clean.trim().to_owned()
}
