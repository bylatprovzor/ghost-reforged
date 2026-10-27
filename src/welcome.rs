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
}

pub const LINES: usize = 6;
const LINE_BYTES: usize = 240;

impl Default for Welcome {
    fn default() -> Self {
        Self {
            enabled: true,
            message: [
                "Welcome to our project w3ladder.net! / Добро пожаловать на наш проект w3ladder.net!",
                "We have a ranked ladder and regular tournaments, join our community! / У нас рейтинговая система, постоянные турниры, присоединяйся к нам в сообщество!",
                "More commands: ?help / Дополнительные команды можно узнать: ?help",
                "#{playername}: {stats}",
            ]
            .join("\n"),
            second: String::new(),
        }
    }
}

impl Welcome {
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
                let mut lines = settings
                    .lines()
                    .into_iter()
                    .map(|l| Welcome::fill(l, &name, &stats));
                let first = lines.next()?;
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
}
