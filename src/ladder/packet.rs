use super::{Error, Profile};

#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub color: u8,
    pub handicap: u8,
    pub race: u8,
    pub difficulty: u8,
    pub team: u8,
}

#[derive(Clone, Debug)]
pub struct Packet {
    pub computers: [Setting; 12],
    pub handicaps: [u8; 4],
}

impl Packet {
    // Version 1 agrees with the map's intro-seed.j. Never clamp statistics.
    pub fn encode(profiles: &[Profile; 4]) -> Result<Self, Error> {
        let mut digits = [0u32; 12];
        for (i, p) in profiles.iter().enumerate() {
            let rank = p.rank.unwrap_or(0);
            if rank >= 1 << 18 || p.wins >= 1 << 12 || p.losses >= 1 << 12 {
                return Err(Error::Capacity);
            }
            if p.slot as usize != i || (p.wins + p.losses < 3 && rank != 0) {
                return Err(Error::InvalidRoster);
            }
            let bits = u64::from(rank) | (u64::from(p.wins) << 18) | (u64::from(p.losses) << 30);
            for digit in 0..3 {
                digits[i * 3 + digit] = ((bits >> (digit * 14)) & 16383) as u32;
            }
        }
        let checksum = digits
            .iter()
            .fold(1, |sum, digit| (sum * 31 + digit) % 9973);
        Ok(Self {
            computers: std::array::from_fn(|i| {
                let d = digits[i];
                Setting {
                    color: (i + 4) as u8,
                    handicap: (d % 100 + 1) as u8,
                    race: [1, 2, 8, 4][((d / 100) % 4) as usize],
                    difficulty: ((d / 400) % 3) as u8,
                    team: (4 + d / 1200) as u8,
                }
            }),
            handicaps: [
                47,
                83,
                (checksum % 100 + 1) as u8,
                (checksum / 100 + 1) as u8,
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_preserves_boundaries_and_never_saturates() {
        let mut profiles = std::array::from_fn(|i| Profile {
            slot: i as u8,
            battletag: format!("Player{i}#1234"),
            wins: 4095,
            losses: 4095,
            rank: Some(262143),
        });
        let packet = Packet::encode(&profiles).unwrap();
        assert!(
            packet
                .computers
                .iter()
                .all(|s| s.handicap == 84 && s.race == 4 && s.difficulty == 1 && s.team == 17)
        );
        profiles[0].wins = 4096;
        assert!(matches!(Packet::encode(&profiles), Err(Error::Capacity)));
        profiles[0].wins = 0;
        profiles[0].losses = 0;
        profiles[0].rank = None;
        assert!(Packet::encode(&profiles).is_ok());
    }
}
