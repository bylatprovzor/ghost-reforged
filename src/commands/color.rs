use crate::protocol::{Command, Lobby, OBSERVER_TEAM, Player};
use serde_json::json;

pub const COLORS: [&str; 24] = [
    "red",
    "blue",
    "teal",
    "purple",
    "yellow",
    "orange",
    "green",
    "pink",
    "gray",
    "lightblue",
    "darkgreen",
    "brown",
    "maroon",
    "navy",
    "turquoise",
    "violet",
    "wheat",
    "peach",
    "mint",
    "lavender",
    "coal",
    "snow",
    "emerald",
    "peanut",
];

pub struct ColorRequest {
    lobby: String,
    player: Player,
    color: u8,
}

fn allowed(lobby: &Lobby, player: &Player, color: u8) -> bool {
    if !player.color_change_enabled
        || player.is_observer
        || player.team == OBSERVER_TEAM
        || color >= 24
    {
        return false;
    }
    if lobby.is_custom_players {
        let choices = &lobby.available_team_colors;
        let group = if choices.is_array() {
            choices.get(player.team as usize)
        } else {
            choices.get(player.team.to_string())
        };
        if !group
            .and_then(|g| g.as_array())
            .is_some_and(|colors| colors.iter().any(|c| c.as_u64() == Some(color as u64)))
            && player.color != Some(color)
        {
            return false;
        }
    }
    !lobby.players.iter().any(|p| {
        p.slot != player.slot
            && p.slot_status == 2
            && !p.is_observer
            && p.team != OBSERVER_TEAM
            && p.color == Some(color)
    })
}

pub(super) fn request(lobby: &Lobby, sender: &str, name: &str) -> (String, Option<ColorRequest>) {
    let normalized = name.to_ascii_lowercase().replace([' ', '-'], "");
    let normalized = if normalized == "grey" {
        "gray"
    } else {
        &normalized
    };
    let Some(color) = COLORS
        .iter()
        .position(|c| *c == normalized)
        .map(|c| c as u8)
    else {
        return (crate::i18n::both(format!("Use ?color <{}>.", COLORS.join("|")), format!("Пишите ?color <{}>.", COLORS.join("|"))), None);
    };
    let Some(player) = lobby
        .players
        .iter()
        .find(|p| p.name == sender && p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
    else {
        return (crate::i18n::both("Player no longer in the lobby.", "Игрок уже вышел из лобби."), None);
    };
    if player.color == Some(color) {
        return (
            crate::i18n::both(format!("{sender} is already {}.", COLORS[color as usize]), format!("У {sender} уже цвет {}.", COLORS[color as usize])),
            None,
        );
    }
    if !allowed(lobby, player, color) {
        return (
            crate::i18n::both("That color is unavailable or locked by the map.", "Этот цвет занят или запрещён картой."),
            None,
        );
    }
    (
        String::new(),
        Some(ColorRequest {
            lobby: lobby.lobby_name.clone(),
            player: player.clone(),
            color,
        }),
    )
}

impl ColorRequest {
    fn current<'a>(&self, lobby: &'a Lobby, same_slot: bool) -> Option<&'a Player> {
        if !lobby.is_host || lobby.lobby_name != self.lobby {
            return None;
        }
        let mut matches = lobby.players.iter().filter(|p| {
            (!same_slot || p.slot == self.player.slot)
                && p.id == self.player.id
                && p.name == self.player.name
                && p.team == self.player.team
                && p.slot_status == 2
                && p.slot_type == 0
                && !p.is_self
                && !p.is_observer
        });
        let player = matches.next()?;
        matches.next().is_none().then_some(player)
    }
    pub(crate) fn confirm(&self, lobby: &Lobby) -> Option<Command> {
        let player = self.current(lobby, true)?;
        allowed(lobby, player, self.color)
            .then(|| Command::new("SetColor", json!({"slot":player.slot,"color":self.color})))
    }
    pub(crate) fn applied(&self, lobby: &Lobby) -> bool {
        // Custom-player maps implement color selection by moving the player
        // to the corresponding slot. Follow identity for the acknowledgement.
        self.current(lobby, false)
            .is_some_and(|p| p.color == Some(self.color))
    }
    pub(crate) fn response(&self, success: bool) -> String {
        if success {
            crate::i18n::both(
                format!("{}: color changed to {}.", self.player.name, COLORS[self.color as usize]),
                format!("{}: цвет изменён на {}.", self.player.name, COLORS[self.color as usize]),
            )
        } else {
            crate::i18n::both(
                format!("{}: color change was not applied. The lobby or map settings may have changed.", self.player.name),
                format!("{}: цвет не изменился. Возможно, поменялись настройки лобби или карты.", self.player.name),
            )
        }
    }
}
