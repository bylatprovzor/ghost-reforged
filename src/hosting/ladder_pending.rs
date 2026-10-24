//! A roster callback acknowledges an edit only when its values are visible.
use super::{Command, Lobby, Packet, Stage, by_color};

enum Expected {
    Color(u8, u8),
    Handicap(u8, u8),
    Computer {
        fixed_slot: Option<u8>,
        setting: crate::ladder::Setting,
    },
}

pub(super) struct PendingEdit {
    pub sequence: u64,
    pub sent_at: u64,
    expected: Option<Expected>,
}

impl PendingEdit {
    pub fn new(
        sequence: u64,
        sent_at: u64,
        command: &Command,
        stage: &Stage,
        packet: Option<&Packet>,
    ) -> Self {
        let number = |key| {
            command.payload[key]
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
        };
        let expected = match command.message {
            "SetColor" => number("slot")
                .zip(number("color"))
                .map(|(s, v)| Expected::Color(s, v)),
            "SetHandicap" => number("slot")
                .zip(number("handicap"))
                .map(|(s, v)| Expected::Handicap(s, v)),
            "SetComputerSlot" => match (stage, packet) {
                (Stage::Write(index), Some(packet)) => {
                    packet
                        .computers
                        .get(*index)
                        .map(|setting| Expected::Computer {
                            fixed_slot: matches!(packet.handicaps[0], 43..=46)
                                .then_some(*index as u8 + 4),
                            setting: setting.clone(),
                        })
                }
                _ => None,
            },
            _ => None,
        };
        Self {
            sequence,
            sent_at,
            expected,
        }
    }

    pub fn confirmed(&self, lobby: &Lobby) -> bool {
        let occupied = |slot| {
            lobby
                .players
                .iter()
                .find(|p| p.slot == slot && p.slot_status == 2 && !p.is_observer)
        };
        match &self.expected {
            Some(Expected::Color(slot, value)) => {
                occupied(*slot).is_some_and(|p| p.color == Some(*value))
            }
            Some(Expected::Handicap(slot, value)) => {
                occupied(*slot).is_some_and(|p| p.handicap == Some(*value))
            }
            Some(Expected::Computer {
                fixed_slot,
                setting,
            }) => {
                // Legacy packets can reorder slots when their team is changed.
                let row = fixed_slot.map_or_else(|| by_color(lobby, setting.color), occupied);
                row.is_some_and(|p| {
                    p.slot_type == 1
                        && p.team == setting.team
                        && p.handicap == Some(setting.handicap)
                        && p.race == Some(setting.race)
                })
            }
            None => false,
        }
    }
}
