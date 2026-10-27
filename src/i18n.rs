//! Lobby chat is shared by everyone and the bot cannot tell a player's
//! language, so every message goes out in English and Russian.

/// Longest single chat line the reply queue sends unsplit.
pub const LINE: usize = 240;

/// "English / Русский" on one line when it fits, otherwise two lines that the
/// reply queue sends as two messages.
pub fn both(en: impl AsRef<str>, ru: impl AsRef<str>) -> String {
    let (en, ru) = (en.as_ref().trim(), ru.as_ref().trim());
    if en.is_empty() || ru.is_empty() || en == ru {
        return if en.is_empty() { ru } else { en }.to_owned();
    }
    let one = format!("{en} / {ru}");
    if one.len() <= LINE {
        one
    } else {
        format!("{en}\n{ru}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pairs_share_a_line_and_long_pairs_split() {
        assert_eq!(both("Hi.", "Привет."), "Hi. / Привет.");
        assert_eq!(both("Same", "Same"), "Same");
        let long = "x".repeat(200);
        assert_eq!(both(&long, "Привет."), format!("{long}\nПривет."));
    }
}
