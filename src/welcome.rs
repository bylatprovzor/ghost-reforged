use crate::protocol::{Command, Lobby};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::VecDeque, time::Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Welcome {
    pub enabled: bool,
    /// One chat message per line (up to `LINES`), sent one second apart.
    pub message: String,
    /// More lines sent after `message`, for example the same greeting in
    /// another language. Empty sends only `message`.
    pub second: String,
    /// Whisper the greeting to the arriving player (`/w Name#1234 …`) instead
    /// of saying it in the lobby. Lines with the player's stats stay public.
    pub whisper: bool,
    /// Lobby gate by w3ladder MMR: players below `min_mmr` or above `max_mmr`
    /// are told and kicked. Lobby admins pass, and so does anyone while the
    /// site cannot be asked.
    pub min_mmr: Option<i32>,
    pub max_mmr: Option<i32>,
    /// Players without a ranked W3Ladder game (no record or `mmr_games` 0)
    /// pass the gate whatever their MMR; `false` turns them away instead.
    pub allow_unrated: bool,
}

pub const LINES: usize = 6;
const LINE_BYTES: usize = 240;

impl Default for Welcome {
    fn default() -> Self {
        Self {
            enabled: true,
            message: [
                "Welcome to our project w3ladder.net! We have a ranked ladder and regular tournaments, join our community! More commands: ?help",
                "Добро пожаловать на наш проект w3ladder.net! У нас рейтинговая система, постоянные турниры, присоединяйся к нам в сообщество!",
                "Дополнительные команды можно узнать: ?help",
                "#{playername}: {stats}",
            ]
            .join("\n"),
            second: String::new(),
            whisper: false,
            min_mmr: None,
            max_mmr: None,
            allow_unrated: true,
        }
    }
}

impl Welcome {
    /// Whether the lobby gate checks ratings at all.
    pub fn gates(&self) -> bool {
        self.min_mmr.is_some() || self.max_mmr.is_some() || !self.allow_unrated
    }

    /// The line telling `name` why this lobby is not theirs, or `None` when
    /// they may stay.
    pub fn turns_away(&self, name: &str, rating: crate::ladder::lookup::Rating) -> Option<String> {
        if rating.unrated() {
            return (!self.allow_unrated).then(|| {
                crate::i18n::both(
                    format!("{name}: this lobby is for rated players. Without a rating, join the other W3Ladder lobby."),
                    format!("{name}: это лобби для игроков с рейтингом. Без рейтинга заходите во второе лобби W3Ladder."),
                )
            });
        }
        let mmr = rating.mmr?;
        let low = self.min_mmr.is_some_and(|m| mmr < m);
        let high = self.max_mmr.is_some_and(|m| mmr > m);
        if !(low || high) {
            return None;
        }
        let range = match (self.min_mmr, self.max_mmr) {
            (Some(a), Some(b)) => format!("{a}-{b}"),
            (Some(a), None) => format!("{a}+"),
            (None, Some(b)) => format!("≤{b}"),
            (None, None) => return None,
        };
        Some(crate::i18n::both(
            format!("{name}: this lobby is for MMR {range} (you have {mmr})."),
            format!("{name}: это лобби для MMR {range} (у вас {mmr})."),
        ))
    }

    pub fn validate(&self) -> Result<(), String> {
        let lines = self.lines();
        if self.message.trim().is_empty()
            || lines.len() > LINES
            || lines.iter().any(|l| l.len() > LINE_BYTES)
            || [&self.message, &self.second]
                .iter()
                .any(|t| t.chars().any(|c| c.is_control() && c != '\n'))
        {
            return Err(format!(
                "Welcome text must have 1 to {LINES} lines of up to {LINE_BYTES} bytes."
            ));
        }
        Ok(())
    }

    fn lines(&self) -> Vec<&str> {
        self.message
            .split('\n')
            .chain(self.second.split('\n'))
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect()
    }

    fn wants_stats(&self) -> bool {
        self.message.contains("{stats}") || self.second.contains("{stats}")
    }

    #[cfg(test)]
    fn render(&self, player: &str, stats: &str) -> String {
        Self::fill(self.lines().first().copied().unwrap_or(""), player, stats)
    }

    fn fill(template: &str, player: &str, stats: &str) -> String {
        let name: String = player
            .chars()
            .filter(|c| !c.is_control())
            .take(32)
            .collect();
        let text = template
            .replace("{stats}", stats)
            .replace("#{playername}", &name);
        let end = (0..=text.len().min(250))
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        // A slash at the beginning would invoke a native chat command.
        let text = &text[..end];
        if text.starts_with('/') {
            format!(" {text}")
        } else {
            text.into()
        }
    }
}

#[derive(Default)]
pub struct Announcements {
    queue: VecDeque<(String, Instant)>,
    /// The remaining welcome lines, one per turn after the first.
    follow: VecDeque<(String, String)>,
    last_sent: Option<u64>,
    last_detected: Option<Instant>,
    /// Lobby and time of the last shared greeting.
    greeted: Option<(String, u64)>,
}

/// Lines naming the player or their stats go to every arrival; the shared
/// greeting is repeated in the same lobby at most this often.
pub const GREETING_REPEAT_SECONDS: u64 = 300;

fn personal(line: &str) -> bool {
    line.contains("#{playername}") || line.contains("{stats}")
}

impl Announcements {
    pub fn joined(&mut self, player: &str) {
        if self.queue.len() < 24 && !self.queue.iter().any(|(name, _)| name == player) {
            self.queue.push_back((player.into(), Instant::now()));
        }
    }

    pub fn left(&mut self, player: &str) {
        self.queue.retain(|(name, _)| name != player);
        self.follow.retain(|(name, _)| name != player);
    }

    /// Call only after the native socket accepted the send. This measures local
    /// submission latency, not delivery to another player's client.
    pub(crate) fn submission_note(&self) -> Option<String> {
        self.last_detected.map(|detected| {
            format!(
                "Welcome submitted; detection-to-send={} ms",
                detected.elapsed().as_millis()
            )
        })
    }

    pub fn next(
        &mut self,
        lobby: &Lobby,
        settings: &Welcome,
        now: u64,
        lookup: &mut crate::ladder::lookup::Lookup,
    ) -> Option<Command> {
        if !settings.enabled {
            self.queue.clear();
            self.follow.clear();
            return None;
        }
        if self.last_sent.is_some_and(|last| now <= last) {
            return None;
        }
        if let Some((_, text)) = self.follow.pop_front() {
            self.last_sent = Some(now);
            return Some(Command::new("SendGameChatMessage", json!({"content":text})));
        }
        while let Some((name, detected)) = self.queue.pop_front() {
            if lobby
                .players
                .iter()
                .any(|p| p.name == name && p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
            {
                let stats = if settings.wants_stats() {
                    match lookup.query(&name, now) {
                        Some(text) => text,
                        None => {
                            self.queue.push_front((name, detected));
                            return None;
                        }
                    }
                } else {
                    String::new()
                };
                self.last_sent = Some(now);
                self.last_detected = Some(detected);
                let whisper = settings.whisper;
                let greet = whisper
                    || self.greeted.as_ref().is_none_or(|(lobby_name, at)| {
                        *lobby_name != lobby.lobby_name
                            || now.saturating_sub(*at) >= GREETING_REPEAT_SECONDS
                    });
                if greet && !whisper {
                    self.greeted = Some((lobby.lobby_name.clone(), now));
                }
                let mut lines = settings
                    .lines()
                    .into_iter()
                    .filter(|l| greet || personal(l))
                    .map(|l| {
                        let text = Welcome::fill(l, &name, &stats);
                        if whisper && !personal(l) {
                            format!("/w {name} {}", text.trim_start())
                        } else {
                            text
                        }
                    });
                let Some(first) = lines.next() else {
                    continue;
                };
                self.follow.extend(lines.map(|l| (name.clone(), l)));
                return Some(Command::new(
                    "SendGameChatMessage",
                    json!({"content":first}),
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joins_use_live_text_once_and_leave_cancels_pending_messages() {
        let roster: Lobby = serde_json::from_value(json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},"players":[
            {"name":"One","slot":1,"slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,"teamChangeEnabled":true},
            {"name":"Two","slot":2,"slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,"teamChangeEnabled":true}
        ]}))
        .unwrap();
        let mut queue = Announcements::default();
        let mut lookup = crate::ladder::lookup::Lookup::default();
        queue.joined("One");
        let detected = Instant::now() - std::time::Duration::from_millis(1500);
        queue.queue.front_mut().unwrap().1 = detected;
        queue.joined("One");
        queue.joined("Two");
        let mut settings = Welcome {
            enabled: true,
            message: "Hi #{playername}".into(),
            second: String::new(),
            whisper: false,
            min_mmr: None,
            max_mmr: None,
            allow_unrated: true,
        };
        assert_eq!(
            queue
                .next(&roster, &settings, 0, &mut lookup)
                .unwrap()
                .payload["content"],
            "Hi One"
        );
        assert_eq!(queue.last_detected, Some(detected));
        let note = queue.submission_note().unwrap();
        assert!(note.starts_with("Welcome submitted; detection-to-send="));
        assert!(!note.contains("One"));
        assert!(queue.next(&roster, &settings, 0, &mut lookup).is_none());
        queue.left("Two");
        assert!(queue.next(&roster, &settings, 1, &mut lookup).is_none());
        queue.joined("Two");
        settings.message = "Hello #{playername}".into();
        assert_eq!(
            queue
                .next(&roster, &settings, 2, &mut lookup)
                .unwrap()
                .payload["content"],
            "Hello Two"
        );
        queue.joined("One");
        settings.enabled = false;
        assert!(queue.next(&roster, &settings, 3, &mut lookup).is_none());
        settings.enabled = true;
        assert!(queue.next(&roster, &settings, 4, &mut lookup).is_none());
    }
    #[test]
    fn template_replaces_names_without_sending_commands_or_unbounded_text() {
        let mut welcome = Welcome::default();
        assert!(
            welcome
                .render("Player", "")
                .starts_with("Welcome to our project w3ladder.net!")
        );
        assert_eq!(welcome.lines().len(), 4);
        assert!(welcome.validate().is_ok());
        welcome.message = "/whisper #{playername}".into();
        assert!(welcome.render("Player", "").starts_with(" /whisper"));
        welcome.message = "#{playername}".repeat(10);
        assert!(welcome.render(&"é".repeat(32), "").len() <= 250);
        welcome.message = "Hello\nWorld".into();
        assert!(welcome.validate().is_ok());
        welcome.message = "Hello\tWorld".into();
        assert!(welcome.validate().is_err());
        welcome.message = vec!["x"; LINES + 1].join("\n");
        assert!(welcome.validate().is_err());
    }
    #[test]
    fn every_line_follows_on_its_own_turn() {
        let roster: Lobby = serde_json::from_value(json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},"players":[
            {"name":"One","slot":1,"slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,"teamChangeEnabled":true}
        ]}))
        .unwrap();
        let mut queue = Announcements::default();
        let mut lookup = crate::ladder::lookup::Lookup::default();
        let settings = Welcome::default();
        let text = |c: Command| c.payload["content"].as_str().unwrap().to_owned();
        queue.joined("One");
        assert!(
            text(queue.next(&roster, &settings, 0, &mut lookup).unwrap())
                .starts_with("Welcome to our project")
        );
        assert!(queue.next(&roster, &settings, 0, &mut lookup).is_none());
        assert!(
            text(queue.next(&roster, &settings, 1, &mut lookup).unwrap())
                .contains("У нас рейтинговая система")
        );
        assert!(text(queue.next(&roster, &settings, 2, &mut lookup).unwrap()).contains("?help"));
        assert!(
            text(queue.next(&roster, &settings, 3, &mut lookup).unwrap())
                .starts_with("One: Wins: ???")
        );
        assert!(queue.next(&roster, &settings, 4, &mut lookup).is_none());
        queue.joined("One");
        queue.next(&roster, &settings, 5, &mut lookup).unwrap();
        queue.left("One");
        assert!(queue.next(&roster, &settings, 6, &mut lookup).is_none());
    }
    #[test]
    fn the_shared_greeting_is_not_repeated_for_every_arrival() {
        let roster: Lobby = serde_json::from_value(json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},"players":[
            {"name":"One","slot":1,"slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,"teamChangeEnabled":true},
            {"name":"Two","slot":2,"slotStatus":2,"slotType":0,"team":1,"isSelf":false,"isObserver":false,"teamChangeEnabled":true}
        ]}))
        .unwrap();
        let settings = Welcome {
            enabled: true,
            message: "Hello\n#{playername}: {stats}".into(),
            second: String::new(),
            whisper: false,
            min_mmr: None,
            max_mmr: None,
            allow_unrated: true,
        };
        let mut queue = Announcements::default();
        let mut lookup = crate::ladder::lookup::Lookup::default();
        let text = |c: Command| c.payload["content"].as_str().unwrap().to_owned();
        queue.joined("One");
        queue.joined("Two");
        assert_eq!(
            text(queue.next(&roster, &settings, 0, &mut lookup).unwrap()),
            "Hello"
        );
        assert!(text(queue.next(&roster, &settings, 1, &mut lookup).unwrap()).starts_with("One: "));
        assert!(text(queue.next(&roster, &settings, 2, &mut lookup).unwrap()).starts_with("Two: "));
        queue.joined("One");
        let later = GREETING_REPEAT_SECONDS + 1;
        assert_eq!(
            text(queue.next(&roster, &settings, later, &mut lookup).unwrap()),
            "Hello"
        );
        // A greeting without personal lines still greets each lobby once.
        let plain = Welcome {
            message: "Hi".into(),
            ..settings.clone()
        };
        assert!(
            text(
                queue
                    .next(&roster, &settings, later + 1, &mut lookup)
                    .unwrap()
            )
            .starts_with("One: ")
        );
        queue.joined("Two");
        assert!(
            queue
                .next(&roster, &plain, later + 5, &mut lookup)
                .is_none()
        );
    }
    #[test]
    fn a_whispered_greeting_reaches_only_the_arrival() {
        let roster: Lobby = serde_json::from_value(json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},"players":[
            {"name":"One#1","slot":1,"slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,"teamChangeEnabled":true},
            {"name":"Two#2","slot":2,"slotStatus":2,"slotType":0,"team":1,"isSelf":false,"isObserver":false,"teamChangeEnabled":true}
        ]}))
        .unwrap();
        let settings = Welcome {
            enabled: true,
            message: "Hello\n#{playername}: rated".into(),
            second: String::new(),
            whisper: true,
            min_mmr: None,
            max_mmr: None,
            allow_unrated: true,
        };
        let mut queue = Announcements::default();
        let mut lookup = crate::ladder::lookup::Lookup::default();
        let text = |c: Command| c.payload["content"].as_str().unwrap().to_owned();
        queue.joined("One#1");
        queue.joined("Two#2");
        assert_eq!(
            text(queue.next(&roster, &settings, 0, &mut lookup).unwrap()),
            "/w One#1 Hello"
        );
        assert_eq!(
            text(queue.next(&roster, &settings, 1, &mut lookup).unwrap()),
            "One#1: rated"
        );
        assert_eq!(
            text(queue.next(&roster, &settings, 2, &mut lookup).unwrap()),
            "/w Two#2 Hello"
        );
        assert_eq!(
            text(queue.next(&roster, &settings, 3, &mut lookup).unwrap()),
            "Two#2: rated"
        );
    }

    #[test]
    fn the_gate_takes_ranges_and_unrated_players_as_configured() {
        use crate::ladder::lookup::Rating;
        let rated = |mmr| Rating {
            mmr: Some(mmr),
            games: Some(12),
        };
        let unrated = Rating {
            mmr: Some(1500),
            games: Some(0),
        };
        // Our lobby: 1500 and up, rated players only.
        let ours = Welcome {
            min_mmr: Some(1500),
            allow_unrated: false,
            ..Welcome::default()
        };
        assert!(ours.gates());
        assert!(ours.turns_away("A#1", rated(1500)).is_none());
        let low = ours.turns_away("A#1", rated(1499)).unwrap();
        assert!(low.contains("MMR 1500+ (you have 1499)") && low.contains("у вас 1499"));
        assert!(
            ours.turns_away("A#1", unrated)
                .unwrap()
                .starts_with("A#1: this lobby is for rated players")
        );
        let no_mmr = Rating {
            mmr: None,
            games: None,
        };
        assert!(ours.turns_away("A#1", no_mmr).is_some());
        // The other lobby: up to 1499, unrated players welcome whatever their MMR.
        let theirs = Welcome {
            max_mmr: Some(1499),
            ..Welcome::default()
        };
        assert!(theirs.turns_away("A#1", unrated).is_none());
        assert!(
            theirs
                .turns_away("A#1", rated(1500))
                .unwrap()
                .contains("≤1499")
        );
        assert!(!Welcome::default().gates());
        // An older profile without the key keeps letting unrated players in.
        let old: Welcome = serde_json::from_str(r#"{"min_mmr":1500}"#).unwrap();
        assert!(old.allow_unrated && old.turns_away("A#1", unrated).is_none());
    }
}
