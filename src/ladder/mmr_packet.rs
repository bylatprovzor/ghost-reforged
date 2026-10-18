use super::{Error, Packet, Profile, Region};

impl Packet {
    // V4 markers: 44 ranked session, 43 preview. Existing markers retain their
    // original W/L interpretation. 5+5 width bits, 60 ticket bits, then four
    // records: rank, zigzag signed MMR, one provisional bit. Zero padding.
    pub fn encode_mmr(
        profiles: &[Profile; 4],
        ticket: Option<&str>,
        regions: &[Region; 4],
    ) -> Result<Self, Error> {
        let token = match ticket {
            None => 0,
            Some(t)
                if t.len() == 15
                    && t.bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
            {
                u64::from_str_radix(t, 16).map_err(|_| Error::InvalidRoster)?
            }
            _ => return Err(Error::InvalidRoster),
        };
        let mut rows = [[0u32; 3]; 4];
        for (i, p) in profiles.iter().enumerate() {
            let mmr = i64::from(p.mmr.ok_or(Error::InvalidRoster)?);
            p.mmr_games.ok_or(Error::InvalidRoster)?;
            if p.slot as usize != i || !p.valid_rank() {
                return Err(Error::InvalidRoster);
            }
            rows[i] = [
                p.rank.unwrap_or(0),
                if mmr >= 0 {
                    (mmr * 2) as u32
                } else {
                    (-mmr * 2 - 1) as u32
                },
                u32::from(p.rank.is_none()),
            ];
        }
        let width = |n: u32| 32 - n.leading_zeros();
        let rank_bits = rows.iter().map(|r| width(r[0])).max().unwrap();
        let mmr_bits = rows.iter().map(|r| width(r[1])).max().unwrap();
        if rank_bits > 23 || mmr_bits > 23 || 74 + 4 * (rank_bits + mmr_bits) > 168 {
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
        push(u64::from(rank_bits), 5);
        push(u64::from(mmr_bits), 5);
        push(token, 60);
        for row in rows {
            for (value, width) in row.into_iter().zip([rank_bits, mmr_bits, 1]) {
                push(u64::from(value), width);
            }
        }
        Ok(Self::with_regions(
            digits,
            regions,
            if ticket.is_some() { 44 } else { 43 },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variable_rank_width_can_use_the_full_payload_capacity() {
        let mut profiles = std::array::from_fn(|i| Profile {
            slot: i as u8,
            battletag: format!("P{i}#1234"),
            rank: Some(262144),
            wins: 0,
            losses: 0,
            mmr: Some(0),
            mmr_games: Some(10),
        });
        assert!(Packet::encode_mmr(&profiles, None, &[Region::Unknown; 4]).is_ok());
        for p in &mut profiles {
            p.rank = Some(8388607);
        }
        assert!(Packet::encode_mmr(&profiles, None, &[Region::Unknown; 4]).is_ok());
        profiles[0].rank = Some(8388608);
        assert!(matches!(
            Packet::encode_mmr(&profiles, None, &[Region::Unknown; 4]),
            Err(Error::Capacity)
        ));
    }

    #[test]
    fn signed_mmr_rank_provisional_and_ticket_round_trip() {
        let values = [1000, -1024, 0, 1499];
        let profiles = std::array::from_fn(|i| Profile {
            slot: i as u8,
            battletag: format!("P{i}#1234"),
            rank: if i == 0 { None } else { Some(i as u32) },
            wins: 0,
            losses: 0,
            mmr: Some(values[i]),
            mmr_games: Some(if i == 0 { 2 } else { 3 }),
        });
        let regions = [
            Region::Europe,
            Region::Americas,
            Region::Asia,
            Region::Unknown,
        ];
        let packet = Packet::encode_mmr(&profiles, Some("0123456789abcde"), &regions).unwrap();
        assert_eq!(packet.handicaps[0], 44);
        let bytes = super::super::fields::decode_bytes(&packet.computers);
        let mut cursor = 0;
        let mut read = |width: usize| {
            let mut n = 0u64;
            for bit in 0..width {
                n |= u64::from((bytes[cursor / 8] >> (cursor % 8)) & 1) << bit;
                cursor += 1;
            }
            n
        };
        let rank_bits = read(5) as usize;
        let mmr_bits = read(5) as usize;
        assert_eq!(read(60), 0x0123456789abcde);
        for (i, value) in values.into_iter().enumerate() {
            assert_eq!(read(rank_bits), i as u64);
            let encoded = read(mmr_bits) as i32;
            let decoded = if encoded % 2 == 0 {
                encoded / 2
            } else {
                -(encoded / 2) - 1
            };
            assert_eq!(decoded, value);
            assert_eq!(read(1), u64::from(i == 0));
        }
        assert_eq!(
            Packet::encode_mmr(&profiles, None, &regions)
                .unwrap()
                .handicaps[0],
            43
        );
        let mut invalid = profiles.clone();
        invalid[0].mmr = None;
        assert!(Packet::encode_mmr(&invalid, None, &regions).is_err());
        invalid[0].mmr = Some(i32::MIN);
        assert!(matches!(
            Packet::encode_mmr(&invalid, None, &regions),
            Err(Error::Capacity)
        ));
    }
}
