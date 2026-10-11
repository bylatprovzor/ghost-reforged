use iced::{
    Border, Color, Shadow, Theme, Vector, gradient,
    widget::{button, container, text_input},
};

pub const BG: Color = Color::from_rgb8(7, 14, 16);
pub const PANEL: Color = Color::from_rgb(0.014, 0.035, 0.055);
pub const LINE: Color = Color::from_rgb8(65, 77, 77);
pub const TEXT: Color = Color::from_rgb8(225, 231, 224);
pub const MUTED: Color = Color::from_rgb8(157, 178, 177);
pub const GOLD: Color = Color::from_rgb8(244, 210, 118);
pub const ACCENT: Color = Color::from_rgb8(87, 190, 148);
pub const ERROR: Color = Color::from_rgb8(245, 126, 113);

pub fn theme(_: &super::app::App) -> Theme {
    Theme::custom(
        "GHost Reforged Citadel",
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: GOLD,
            success: ACCENT,
            warning: GOLD,
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
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}

pub fn plain(_: &Theme) -> container::Style {
    container::Style {
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn activity_highlight(_: &Theme) -> container::Style {
    container::Style {
        background: Some(GOLD.scale_alpha(0.09).into()),
        text_color: Some(GOLD),
        border: Border {
            color: GOLD.scale_alpha(0.3),
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    metal(status, true)
}
pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    metal(status, false)
}

pub fn window_button(_: &Theme, _: button::Status) -> button::Style {
    button::Style::default()
}

pub fn inset(_: &Theme, _: button::Status) -> button::Style {
    button::Style {
        background: Some(Color::from_rgb8(5, 13, 21).into()),
        text_color: TEXT,
        ..Default::default()
    }
}

fn metal(status: button::Status, selected: bool) -> button::Style {
    let disabled = status == button::Status::Disabled;
    let top = if selected {
        Color::from_rgb8(5, 34, 216)
    } else {
        Color::from_rgb8(5, 22, 128)
    };
    let bottom = if selected {
        Color::from_rgb8(0, 4, 89)
    } else {
        BG
    };
    button::Style {
        background: Some(
            gradient::Linear::new(std::f32::consts::FRAC_PI_2)
                .add_stop(0.0, top)
                .add_stop(1.0, bottom)
                .into(),
        ),
        text_color: if disabled {
            MUTED.scale_alpha(0.5)
        } else {
            GOLD
        },
        border: Border {
            color: if selected {
                GOLD.scale_alpha(0.75)
            } else {
                LINE
            },
            width: 1.0,
            radius: 3.0.into(),
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.45),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 3.0,
        },
        ..Default::default()
    }
}

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.background = Color::from_rgb8(6, 14, 17).into();
    style.border = Border {
        color: if matches!(status, text_input::Status::Focused { .. }) {
            GOLD
        } else {
            LINE
        },
        width: 1.0,
        radius: 2.0.into(),
    };
    style.value = TEXT;
    style.placeholder = MUTED;
    style
}

pub fn navigation(_: &Theme, status: button::Status) -> button::Style {
    let mut style = metal(status, false);
    style.background = Some(Color::from_rgb8(8, 16, 22).into());
    style.text_color = TEXT;
    style
}

pub fn scrollbar(
    theme: &Theme,
    status: iced::widget::scrollable::Status,
) -> iced::widget::scrollable::Style {
    let mut style = iced::widget::scrollable::default(theme, status);
    let rail = &mut style.vertical_rail;
    rail.background = Some(Color::from_rgb8(3, 9, 20).into());
    rail.border = Border {
        color: LINE,
        width: 1.0,
        radius: 1.0.into(),
    };
    rail.scroller.background = gradient::Linear::new(0.0)
        .add_stop(0.0, Color::from_rgb8(0, 9, 90))
        .add_stop(0.5, Color::from_rgb8(9, 46, 225))
        .add_stop(1.0, Color::from_rgb8(0, 9, 90))
        .into();
    rail.scroller.border = Border {
        color: Color::from_rgb8(76, 103, 147),
        width: 1.0,
        radius: 1.0.into(),
    };
    style
}
