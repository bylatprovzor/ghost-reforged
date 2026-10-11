use iced::{
    Border, Color, Theme,
    widget::{button, container},
};

pub const BG: Color = Color::from_rgb8(17, 20, 23);
pub const PANEL: Color = Color::from_rgb8(24, 29, 33);
pub const LINE: Color = Color::from_rgb8(47, 56, 61);
pub const TEXT: Color = Color::from_rgb8(232, 235, 237);
pub const MUTED: Color = Color::from_rgb8(152, 164, 172);
pub const ACCENT: Color = Color::from_rgb8(113, 196, 174);
pub const ERROR: Color = Color::from_rgb8(234, 158, 144);

pub fn theme(_: &super::app::App) -> Theme {
    Theme::custom(
        "GHost Reforged Graphite",
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: ACCENT,
            warning: Color::from_rgb8(216, 185, 120),
            danger: ERROR,
        },
    )
}

pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PANEL.into()),
        text_color: Some(TEXT),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

pub fn plain(_: &Theme) -> container::Style {
    container::Style {
        background: Some(BG.into()),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn activity_highlight(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color { a: 0.12, ..ACCENT }.into()),
        text_color: Some(ACCENT),
        border: Border {
            color: Color { a: 0.4, ..ACCENT },
            width: 1.0,
            radius: 5.0.into(),
        },
        ..Default::default()
    }
}

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let mut s = button::primary(theme, status);
    s.border.radius = 5.0.into();
    s.text_color = BG;
    s
}

pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(match status {
            button::Status::Hovered => LINE.into(),
            _ => PANEL.into(),
        }),
        text_color: match status {
            button::Status::Disabled => MUTED,
            _ => TEXT,
        },
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 5.0.into(),
        },
        ..Default::default()
    }
}
