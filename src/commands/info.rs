use super::{ChatCommand, CommandSettings};
use crate::{protocol::Lobby, stats::History};

pub(super) fn help(settings: &CommandSettings, argument: &str) -> String {
    if !argument.is_empty() {
        return ChatCommand::parse(argument)
            .filter(|c| settings.enabled(*c))
            .map(|c| {
                let name = match c.short() {
                    Some(short) => format!("?{} (?{short})", c.name()),
                    None => format!("?{}", c.name()),
                };
                crate::i18n::both(format!("{name}: {}", c.description()), format!("{name}: {}", c.description_ru()))
            })
            .unwrap_or_else(|| crate::i18n::both("Unknown or disabled command. Use ?help.", "Нет такой команды. Пишите ?help."));
    }
    let list = ChatCommand::ALL
        .into_iter()
        .filter(|c| settings.enabled(*c))
        .map(|c| match c.short() {
            Some(short) => format!("{}({short})", c.name()),
            None => c.name().to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    crate::i18n::both(
        format!("Commands: {list}. Start each command with ?. ?help <command> for details."),
        "Команды пишутся через ? в начале. ?help <команда> — подробнее.",
    )
}

pub(super) fn regions(lobby: &Lobby, argument: &str) -> String {
    let players: Vec<_> = lobby
        .players
        .iter()
        .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.name.trim().is_empty())
        .filter(|p| {
            argument.is_empty()
                || p.name.eq_ignore_ascii_case(argument)
                || p.name
                    .split('#')
                    .next()
                    .is_some_and(|s| s.eq_ignore_ascii_case(argument))
        })
        .collect();
    if players.is_empty() {
        return crate::i18n::both("Player not found. Use ?from [full BattleTag].", "Игрок не найден. Пишите ?from [полный BattleTag].");
    }
    if !argument.is_empty() && players.len() > 1 {
        return crate::i18n::both("That name is ambiguous. Use the full BattleTag.", "Таких имён несколько. Напишите полный BattleTag.");
    }
    let rows = players
        .iter()
        .map(|p| {
            let region = match p.player_region.to_ascii_lowercase().as_str() {
                "eu" => "Europe",
                "usw" | "use" | "us" => "Americas",
                "kr" => "Asia",
                "cn" => "China",
                _ => match p.gateway_id {
                    Some(10) => "Americas",
                    Some(20) => "Europe",
                    Some(30) => "Asia",
                    _ => "unknown",
                },
            };
            format!("{}: {region}", p.name)
        })
        .collect::<Vec<_>>()
        .join("; ");
    crate::i18n::both(format!("Battle.net regions: {rows}."), format!("Регионы Battle.net: {rows}."))
}

pub(super) fn map(lobby: &Lobby) -> String {
    let file = lobby
        .map_data
        .map_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    if lobby.map_data.map_name.trim().is_empty() {
        format!("Map: {file}")
    } else {
        format!("Map: {} | {file}", lobby.map_data.map_name)
    }
}

pub(super) fn stats(history: Option<&History>, sender: &str, argument: &str) -> String {
    let Some(history) = history else {
        return crate::i18n::both("Statistics are unavailable in this session.", "Статистика сейчас недоступна.");
    };
    if argument.is_empty() {
        return history.describe(sender);
    }
    match history.resolve(argument) {
        Ok(name) => history.describe(&name),
        Err(message) => message.into(),
    }
}

pub(super) fn ladder_target(
    lobby: &Lobby,
    sender: &str,
    argument: &str,
) -> Result<String, &'static str> {
    if argument.is_empty() {
        return Ok(sender.into());
    }
    let mut matches = lobby
        .players
        .iter()
        .filter(|p| p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
        .filter(|p| {
            p.name.eq_ignore_ascii_case(argument)
                || p.name
                    .split('#')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case(argument))
        });
    let player = matches
        .next()
        .ok_or("Player not found. Use ?stats [full BattleTag]. / Игрок не найден. Пишите ?stats [полный BattleTag].")?;
    if matches.next().is_some() {
        return Err("That name is ambiguous. Use the full BattleTag. / Таких имён несколько. Напишите полный BattleTag.");
    }
    Ok(player.name.clone())
}
