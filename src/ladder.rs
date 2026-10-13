//! Public ladder reads and the versioned lobby packet understood by the map.
mod api;
mod packet;
mod session;
pub use api::fetch;
pub use packet::{Packet, Setting};
use serde::Deserialize;
pub use session::{Prepared, prepare};

pub const MAP_NAME: &str = "Survival Chaos Reforged Rank";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Profile {
    pub slot: u8,
    pub battletag: String,
    pub wins: u32,
    pub losses: u32,
    pub rank: Option<u32>,
}

impl Profile {
    pub fn announcement(&self) -> String {
        format!(
            "{}: Wins: {}, Loss: {}, Rank: {}",
            self.battletag,
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
    clean.trim().eq_ignore_ascii_case(MAP_NAME)
}
