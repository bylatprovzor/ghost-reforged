//! Stable native lobby names shared by graphical and console launchers.
use std::path::Path;

pub fn pattern(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut base = stem.trim();
    // Download indexes and melee player counts are not part of the title.
    loop {
        if let Some((prefix, rest)) = base.split_once('_')
            && !prefix.is_empty()
            && prefix.bytes().all(|c| c.is_ascii_digit())
        {
            base = rest.trim_start();
            continue;
        }
        if let Some(first) = base.chars().next()
            && matches!(first, '(' | '[')
        {
            let end = if first == '(' { ')' } else { ']' };
            if let Some((count, rest)) = base[1..].split_once(end)
                && !count.is_empty()
                && count.bytes().all(|c| c.is_ascii_digit())
            {
                base = rest.trim_start_matches([' ', '_', '-']);
                continue;
            }
        }
        break;
    }
    let chars: Vec<_> = base.chars().collect();
    let mut words = String::new();
    for (index, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            words.push(' ');
            continue;
        }
        if c.is_uppercase()
            && index > 0
            && (chars[index - 1].is_lowercase()
                || chars[index - 1].is_uppercase()
                    && chars.get(index + 1).is_some_and(|c| c.is_lowercase()))
        {
            words.push(' ');
        }
        words.push(c);
    }
    let mut tokens: Vec<_> = words
        .split_whitespace()
        .take_while(|word| !version_marker(word))
        .collect();
    while tokens.last().is_some_and(|word| {
        word.eq_ignore_ascii_case("reborn") || word.eq_ignore_ascii_case("reforged")
    }) {
        tokens.pop();
    }
    let mut title = tokens.join(" ");
    // Reserve room for " #" and the largest persisted u32 counter.
    while title.len() > 19 {
        title.pop();
    }
    let title = title.trim();
    format!("{} #{{n}}", if title.is_empty() { "Game" } else { title })
}

fn version_marker(word: &str) -> bool {
    let word = word.to_ascii_lowercase();
    [
        "v", "ver", "version", "fix", "fixed", "beta", "alpha", "patch", "update",
    ]
    .iter()
    .any(|prefix| {
        word == *prefix
            || word
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with(|c: char| c.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_titles_remove_download_indexes_versions_and_edition_suffixes() {
        for (file, expected) in [
            ("SurvivalChaosReborn.w3x", "Survival Chaos #{n}"),
            (
                "5473_SurvivalChaosReborn_v1.59_fix1.w3x",
                "Survival Chaos #{n}",
            ),
            ("(2)TerenasStand_LV.w3x", "Terenas Stand LV #{n}"),
            ("123_[4]CastleFight_v2.0.w3x", "Castle Fight #{n}"),
            ("VampireHunters.w3x", "Vampire Hunters #{n}"),
        ] {
            assert_eq!(pattern(Path::new(file)), expected);
        }
    }
    #[test]
    fn generated_names_fit_the_native_limit_without_cutting_unicode() {
        for file in [
            "VeryLongMapNameThatExceedsTheLobbyLimit.w3x",
            "ÉléphantsÉternelsÉpiques.w3x",
            "____.w3x",
        ] {
            let generated = pattern(Path::new(file));
            assert!(generated.replace("{n}", "4294967295").len() <= 31);
            assert!(!generated.trim_start().starts_with('#'));
        }
    }
}
