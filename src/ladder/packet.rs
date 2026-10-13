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
    pub fn encode_session(profiles: &[Profile; 4], ticket: &str) -> Result<Self, Error> {
        // V2: 3 widths (4 bits each), 60 ticket bits, then four variable-width
        // rank/wins/losses records. V1 retains its original interpretation.
        Self::encode(profiles)?;
        if ticket.len() != 15
            || !ticket
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(Error::InvalidRoster);
        }
        let token = u64::from_str_radix(ticket, 16).map_err(|_| Error::InvalidRoster)?;
        let width = |n: u32| 32 - n.leading_zeros();
        let widths = [
            profiles
                .iter()
                .map(|p| width(p.rank.unwrap_or(0)))
                .max()
                .unwrap(),
            profiles.iter().map(|p| width(p.wins)).max().unwrap(),
            profiles.iter().map(|p| width(p.losses)).max().unwrap(),
        ];
        if widths[0] > 15 || 72 + 4 * widths.iter().sum::<u32>() > 168 {
            return Err(Error::Capacity);
        }
        let mut digits = [0u32; 12];
        let mut cursor = 0;
        let mut push = |value: u64, width: u32| {
            for bit in 0..width {
                digits[cursor / 14] |= (((value >> bit) & 1) as u32) << (cursor % 14);
                cursor += 1;
            }
        };
        for w in widths {
            push(u64::from(w), 4);
        }
        push(token, 60);
        for p in profiles {
            for (value, w) in [p.rank.unwrap_or(0), p.wins, p.losses]
                .into_iter()
                .zip(widths)
            {
                push(u64::from(value), w);
            }
        }
        Ok(Self::from_digits(digits, 84))
    }

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
        Ok(Self::from_digits(digits, 83))
    }

    fn from_digits(digits: [u32; 12], version_marker: u8) -> Self {
        let checksum = digits
            .iter()
            .fold(1, |sum, digit| (sum * 31 + digit) % 9973);
        Self {
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
                version_marker,
                (checksum % 100 + 1) as u8,
                (checksum / 100 + 1) as u8,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_packet_preserves_the_ticket_and_every_profile_bit() {
        let profiles = [(3, 258, 42), (17, 213, 87), (64, 180, 120), (0, 1, 1)]
            .map(|(rank, wins, losses)| (rank, wins, losses));
        let profiles = std::array::from_fn(|i| Profile {
            slot: i as u8,
            battletag: format!("Player{i}#1234"),
            rank: (profiles[i].0 != 0).then_some(profiles[i].0),
            wins: profiles[i].1,
            losses: profiles[i].2,
        });
        let packet = Packet::encode_session(&profiles, "fedcba987654321").unwrap();
        assert_eq!(packet.handicaps[1], 84);
        let digits: Vec<u32> = packet
            .computers
            .iter()
            .map(|s| {
                u32::from(s.handicap - 1)
                    + 100 * [1, 2, 8, 4].iter().position(|r| *r == s.race).unwrap() as u32
                    + 400 * u32::from(s.difficulty)
                    + 1200 * u32::from(s.team - 4)
            })
            .collect();
        let mut cursor = 0;
        let mut read = |count: u32| {
            let mut n = 0u64;
            for bit in 0..count {
                n |= u64::from((digits[cursor / 14] >> (cursor % 14)) & 1) << bit;
                cursor += 1;
            }
            n
        };
        let widths = [read(4) as u32, read(4) as u32, read(4) as u32];
        assert_eq!(widths, [7, 9, 7]);
        // JASS reads three positive 20-bit integers, never a signed 60-bit number.
        let low = read(20);
        let mid = read(20);
        let high = read(20);
        assert_eq!(format!("{high:05x}{mid:05x}{low:05x}"), "fedcba987654321");
        for p in &profiles {
            assert_eq!(
                [read(widths[0]), read(widths[1]), read(widths[2])],
                [
                    u64::from(p.rank.unwrap_or(0)),
                    u64::from(p.wins),
                    u64::from(p.losses)
                ]
            );
        }
        assert!(Packet::encode_session(&profiles, "secret-not-hex").is_err());
        let mut overflow = profiles;
        overflow[0].rank = Some(4095);
        overflow[0].wins = 4095;
        assert!(matches!(
            Packet::encode_session(&overflow, "000000000000001"),
            Err(Error::Capacity)
        ));
    }

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
