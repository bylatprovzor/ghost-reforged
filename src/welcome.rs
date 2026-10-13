use crate::protocol::{Command, Lobby};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Welcome {
    pub enabled: bool,
    pub message: String,
}

impl Default for Welcome {
    fn default() -> Self {
        Self {
            enabled: true,
            message: "Welcome to this automatic lobby #{playername}, please type ?help to see the different options".into(),
        }
    }
}

impl Welcome {
    pub fn validate(&self) -> Result<(), String> {
        if self.message.trim().is_empty()
            || self.message.chars().any(char::is_control)
            || self.message.len() > 200
        {
            return Err("Welcome message must contain 1 to 200 bytes on one line.".into());
        }
        Ok(())
    }

    fn render(&self, player: &str, stats: &str) -> String {
        let name: String = player
            .chars()
            .filter(|c| !c.is_control())
            .take(32)
            .collect();
        let text = self
            .message
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
    queue: VecDeque<String>,
    last_sent: Option<u64>,
}

impl Announcements {
    pub fn joined(&mut self, player: &str) {
        if self.queue.len() < 24 && !self.queue.iter().any(|name| name == player) {
            self.queue.push_back(player.into());
        }
    }

    pub fn left(&mut self, player: &str) {
        self.queue.retain(|name| name != player);
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
            return None;
        }
        if self.last_sent.is_some_and(|last| now <= last) {
            return None;
        }
        while let Some(name) = self.queue.pop_front() {
            if lobby
                .players
                .iter()
                .any(|p| p.name == name && p.slot_status == 2 && p.slot_type == 0 && !p.is_self)
            {
                let stats = if settings.message.contains("{stats}") {
                    match lookup.query(&name, now) {
                        Some(text) => text,
                        None => {
                            self.queue.push_front(name);
                            return None;
                        }
                    }
                } else {
                    String::new()
                };
                self.last_sent = Some(now);
                return Some(Command::new(
                    "SendGameChatMessage",
                    json!({"content":settings.render(&name, &stats)}),
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
        queue.joined("One");
        queue.joined("Two");
        let mut settings = Welcome {
            enabled: true,
            message: "Hi #{playername}".into(),
        };
        assert_eq!(
            queue
                .next(&roster, &settings, 0, &mut lookup)
                .unwrap()
                .payload["content"],
            "Hi One"
        );
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
        assert!(welcome.render("Player", "").contains("lobby Player,"));
        welcome.message = "/whisper #{playername}".into();
        assert!(welcome.render("Player", "").starts_with(" /whisper"));
        welcome.message = "#{playername}".repeat(10);
        assert!(welcome.render(&"é".repeat(32), "").len() <= 250);
        welcome.message = "Hello\nWorld".into();
        assert!(welcome.validate().is_err());
    }
}
