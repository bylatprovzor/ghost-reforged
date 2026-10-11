use super::{
    app::{Field, Message},
    art::{self, Icon},
    effects,
    frame_art::Frame,
    palette as p,
};
use iced::{
    Alignment, Element, Fill,
    widget::{button, column, container, row, text, text_input},
};

pub fn muted<'a>(value: impl Into<String>) -> iced::widget::Text<'a> {
    text(value.into()).size(14).color(p::MUTED)
}

pub fn label<'a>(value: &'a str, icon: Icon) -> Element<'a, Message> {
    let _ = icon;
    text(value).size(16).color(p::GOLD).into()
}

pub fn field<'a>(name: &'a str, value: &'a str, kind: Field) -> Element<'a, Message> {
    field_enabled(name, value, kind, true)
}

pub fn field_enabled<'a>(
    name: &'a str,
    value: &'a str,
    kind: Field,
    enabled: bool,
) -> Element<'a, Message> {
    let icon = match kind {
        Field::Bot | Field::Title => Icon::User,
        Field::Pattern => Icon::Game,
        Field::Number => Icon::Number,
        Field::Players => Icon::Players,
        _ => Icon::Scroll,
    };
    let input = text_input(name, value)
        .padding([8, 14])
        .size(16)
        .style(p::input);
    let input = if enabled {
        input.on_input(move |value| Message::Edit(kind, value))
    } else {
        input
    };
    column![label(name, icon), effects::skin(input, Frame::Input)]
        .spacing(2)
        .into()
}

pub fn action(name: &'static str, message: Message, enabled: bool) -> Element<'static, Message> {
    optional_action(name, enabled.then_some(message))
}

pub fn optional_action(name: &'static str, message: Option<Message>) -> Element<'static, Message> {
    let enabled = message.is_some();
    let icon = match name {
        "New" | "New profile" => None,
        "Browse" => None,
        "Use map name" => None,
        "Save profile" => Some(Icon::Save),
        "Host" => Some(Icon::Play),
        "Force start" => Some(Icon::Swords),
        _ => None,
    };
    let mut content = row![].spacing(7).align_y(Alignment::Center);
    if let Some(icon) = icon {
        content = content.push(art::icon(icon, 27.0));
    }
    content = content.push(text(name).size(16));
    let primary = matches!(name, "Host" | "Force start" | "Stop Hosting");
    effects::skin(
        effects::hover(
            button(content)
                .padding([if primary { 13 } else { 7 }, 16])
                .width(if primary {
                    iced::Length::Fixed(164.0)
                } else {
                    iced::Length::Shrink
                })
                .style(p::secondary)
                .on_press_maybe(message),
            enabled,
        ),
        Frame::Button,
    )
}

pub fn heading<'a>(name: &'a str, icon: Icon) -> Element<'a, Message> {
    let title = text(name).font(art::LABEL).size(27).color(p::GOLD);
    let content: Element<'_, Message> = if name == "Auto Host" {
        title.into()
    } else {
        row![art::icon(icon, 36.0), title]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
    };
    let bar = container(content).padding([16, 24]).width(Fill);
    column![bar, art::separator()].spacing(0).into()
}
