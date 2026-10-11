use super::{
    app::{App, Message},
    palette as p,
};
use iced::{
    Alignment, Color, Element, Fill, Length,
    widget::{button, column, container, image, mouse_area, opaque, row, space, stack, text},
};

fn picture(app: &App, height: impl Into<Length>) -> Element<'_, Message> {
    if let Some(frame) = &app.preview.image {
        // Handle clones share the same pixel buffer with the thumbnail.
        image(frame.clone())
            .width(Fill)
            .height(height)
            .content_fit(iced::ContentFit::Contain)
            .into()
    } else {
        container(
            text(
                app.preview
                    .error
                    .as_deref()
                    .unwrap_or("Waiting for Warcraft III…"),
            )
            .size(13)
            .color(p::MUTED),
        )
        .center_x(Fill)
        .center_y(height)
        .into()
    }
}

fn age(app: &App) -> String {
    app.preview
        .updated
        .map(|at| format!("{}s ago", at.elapsed().as_secs()))
        .unwrap_or_default()
}

pub fn thumbnail(app: &App) -> Element<'_, Message> {
    let caption = if app.preview.error.is_some() && app.preview.image.is_some() {
        "Last capture"
    } else {
        "Game view"
    };
    button(
        column![
            row![
                text(caption).size(13).color(p::MUTED),
                space().width(Fill),
                text(age(app)).size(13).color(p::MUTED)
            ],
            picture(app, 185),
        ]
        .spacing(8),
    )
    .padding(10)
    .width(Fill)
    .style(p::secondary)
    .on_press_maybe(app.preview.image.is_some().then_some(Message::OpenPreview))
    .into()
}

pub fn modal<'a>(app: &'a App, base: Element<'a, Message>) -> Element<'a, Message> {
    let backdrop =
        mouse_area(
            container(space())
                .width(Fill)
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.8).into()),
                    ..Default::default()
                }),
        )
        .on_press(Message::ClosePreview);
    let content = container(
        column![
            row![
                text("Game view").size(20),
                space().width(Fill),
                text(age(app)).size(13).color(p::MUTED),
                button(text("Close").size(13))
                    .padding([9, 14])
                    .style(p::secondary)
                    .on_press(Message::ClosePreview),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
            picture(app, Fill),
        ]
        .spacing(14),
    )
    .padding(16)
    .width(Fill)
    .height(Fill)
    .max_width(1320)
    .max_height(850)
    .style(p::panel);
    stack![
        base,
        backdrop,
        container(opaque(content))
            .center_x(Fill)
            .center_y(Fill)
            .padding(32)
    ]
    .into()
}
