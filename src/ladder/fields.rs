use super::Setting;

// Twelve base-1200 settings and twelve distinct colors from 4..23 carry
// 168 bits without changing a gameplay team. Slots 4..15 identify the rows.
pub(super) fn encode(mut words: [u32; 12]) -> [Setting; 12] {
    let settings: [u32; 12] = std::array::from_fn(|_| divide(&mut words, 1200));
    let mut colors: Vec<u8> = (4..24).collect();
    std::array::from_fn(|i| {
        let index = divide(&mut words, colors.len() as u32) as usize;
        let d = settings[i];
        Setting {
            color: colors.remove(index),
            handicap: (d % 100 + 1) as u8,
            race: [1, 2, 8, 4][((d / 100) % 4) as usize],
            difficulty: (d / 400) as u8,
            team: i as u8 % 4,
        }
    })
}

fn divide(words: &mut [u32; 12], base: u32) -> u32 {
    let mut remainder = 0;
    for word in words.iter_mut().rev() {
        let value = remainder * 16384 + *word;
        *word = value / base;
        remainder = value % base;
    }
    remainder
}

#[cfg(test)]
pub(super) fn decode_bytes(fields: &[Setting; 12]) -> [u8; 21] {
    // Independent byte-based reconstruction, also used by the packet tests.
    let mut bytes = [0u8; 21];
    let mut multiply = |base: u32, mut carry: u32| {
        for byte in &mut bytes {
            carry += u32::from(*byte) * base;
            *byte = carry as u8;
            carry >>= 8;
        }
        assert_eq!(carry, 0, "packet exceeds 168 bits");
    };
    let mut colors: Vec<u8> = (4..24).collect();
    let indices: [u32; 12] = std::array::from_fn(|i| {
        let index = colors.iter().position(|c| *c == fields[i].color).unwrap();
        colors.remove(index);
        index as u32
    });
    for i in (0..12).rev() {
        multiply(20 - i as u32, indices[i]);
    }
    for s in fields.iter().rev() {
        let digit = u32::from(s.handicap - 1)
            + 100 * [1, 2, 8, 4].iter().position(|r| *r == s.race).unwrap() as u32
            + 400 * u32::from(s.difficulty);
        multiply(1200, digit);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_payload_capacity_preserves_four_teams_and_unique_colors() {
        let mut cases = vec![[0; 12], [16383; 12]];
        for bit in 0..168 {
            let mut words = [0; 12];
            words[bit / 14] = 1 << (bit % 14);
            cases.push(words);
        }
        for words in cases {
            let fields = encode(words);
            for (i, s) in fields.iter().enumerate() {
                assert_eq!(s.team, i as u8 % 4);
                assert!((1..=100).contains(&s.handicap));
                assert!(s.difficulty < 3);
            }
            let bytes = decode_bytes(&fields);
            for bit in 0..168 {
                assert_eq!(
                    (bytes[bit / 8] >> (bit % 8)) & 1,
                    ((words[bit / 14] >> (bit % 14)) & 1) as u8
                );
            }
        }
    }
}
