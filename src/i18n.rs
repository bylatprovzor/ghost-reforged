//! Lobby chat is shared by everyone and the bot cannot tell a player's
//! language, so every message goes out in English and Russian.

/// English first, Russian on the next line: the reply queue sends each line
/// as its own chat message.
pub fn both(en: impl AsRef<str>, ru: impl AsRef<str>) -> String {
    let (en, ru) = (en.as_ref().trim(), ru.as_ref().trim());
    if en.is_empty() || ru.is_empty() || en == ru {
        return if en.is_empty() { ru } else { en }.to_owned();
    }
    format!("{en}\n{ru}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_follows_on_its_own_line() {
        assert_eq!(both("Hi.", "Привет."), "Hi.\nПривет.");
        assert_eq!(both("Same", "Same"), "Same");
        assert_eq!(both("", "Привет."), "Привет.");
    }
}
