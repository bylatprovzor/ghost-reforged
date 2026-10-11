pub(super) use super::widgets::{action, field, muted};
use super::{
    app::{App, Field, Message, Page, WindowAction},
    art::{self, Icon},
    backdrop_view as bg, effects,
    frame_art::Frame,
    palette as p,
    process::GameState,
};
use iced::{
    Alignment, Border, Color, Element, Fill,
    widget::{
        button, column, container, mouse_area, row, rule, scrollable, space, stack, text, tooltip,
    },
};

pub fn view(app: &App) -> Element<'_, Message> {
    let brand = effects::skin(
        container(
            row![
                art::ghost(43.0),
                text("GHost Reforged")
                    .font(art::DISPLAY)
                    .color(p::GOLD)
                    .size(29)
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        )
        .padding([15, 31])
        .width(409)
        .height(94)
        .style(p::panel),
        Frame::Header,
    );
    let title = mouse_area(container(brand).padding([27, 12]).width(Fill).height(160))
        .on_press(Message::Window(WindowAction::Drag))
        .on_double_click(Message::Window(WindowAction::Maximize));
    let header = row![title, super::chrome::controls()]
        .align_y(Alignment::Start)
        .height(bg::HEADER);
    let mut navigation = column![].spacing(8);
    for (page, name, icon) in [
        (Page::Hosting, "Hosting", Icon::Tower),
        (Page::Profiles, "Profiles", Icon::People),
        (Page::Options, "Options", Icon::Gear),
        (Page::Activity, "Activity", Icon::Scroll),
    ] {
        navigation = navigation.push(effects::skin(
            effects::hover(
                button(
                    row![art::icon(icon, 39.0), text(name).font(art::LABEL).size(18)]
                        .spacing(13)
                        .align_y(Alignment::Center),
                )
                .padding([12, 21])
                .height(64)
                .width(Fill)
                .style(if app.page == page {
                    p::primary
                } else {
                    p::navigation
                })
                .on_press(Message::Page(page)),
                true,
            ),
            if app.page == page {
                Frame::SelectedNavigation
            } else {
                Frame::Navigation
            },
        ));
    }
    let color = match app.game {
        GameState::Stopped => p::ERROR,
        GameState::Background | GameState::Visible => p::ACCENT,
        GameState::Multiple => p::GOLD,
        _ => p::MUTED,
    };
    let led = container(space())
        .width(15)
        .height(15)
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: Border {
                color: Color::from_rgb8(13, 19, 17),
                width: 3.0,
                radius: 8.0.into(),
            },
            shadow: iced::Shadow {
                color: color.scale_alpha(0.5),
                blur_radius: 5.0,
                ..Default::default()
            },
            ..Default::default()
        });
    let status = tooltip(
        effects::skin(
            container(
                row![led, text(app.game.label()).size(16)]
                    .spacing(10)
                    .align_y(Alignment::Center),
            )
            .padding([16, 23])
            .width(Fill)
            .style(p::panel),
            Frame::Navigation,
        ),
        container(text(app.game.tooltip()).size(13))
            .padding(10)
            .style(p::panel),
        tooltip::Position::Right,
    );
    navigation = navigation.push(space().height(Fill)).push(status);
    let sidebar = container(navigation)
        .width(bg::SIDEBAR)
        .height(Fill)
        .padding(iced::Padding {
            left: 7.0,
            right: 7.0,
            top: 8.0,
            bottom: 0.0,
        });
    let body = match app.page {
        Page::Hosting => super::hosting_view::view(app),
        Page::Profiles => profiles(app),
        Page::Activity => activity(app),
        Page::Options => super::options_view::view(app),
    };
    let content = row![
        sidebar,
        container(effects::slide(body, app.motion.page.value(app.motion.now)))
            .width(Fill)
            .height(Fill)
    ]
    .spacing(bg::SIDEBAR_GAP)
    .height(Fill);
    let foreground = container(column![header, content])
        .padding(iced::Padding {
            right: bg::RIGHT,
            bottom: bg::BOTTOM,
            ..Default::default()
        })
        .width(Fill)
        .height(Fill)
        .style(p::plain);
    let background = app.backdrop.frame.clone().unwrap_or_else(art::backdrop);
    let base = stack![
        bg::view(
            background,
            bg::Coverage::for_page(app.page, app.motion.page.value(app.motion.now))
        ),
        foreground
    ]
    .into();
    if app.motion.modal_visible() {
        stack![
            super::preview_view::modal(app, base),
            container(row![space().width(Fill), super::chrome::controls()]).padding(
                iced::Padding {
                    right: 20.0,
                    ..Default::default()
                }
            )
        ]
        .into()
    } else {
        base
    }
}
fn profiles(app: &App) -> Element<'_, Message> {
    let mut items = column![].spacing(16);
    for (index, profile) in app.profiles.items.iter().enumerate() {
        items = items.push(
            container(
                row![
                    column![text(&profile.title).size(17), muted(profile.map_label())]
                        .spacing(6)
                        .width(Fill),
                    action(
                        if index == app.profiles.selected {
                            "Selected"
                        } else {
                            "Load"
                        },
                        Message::SelectProfile(index),
                        index != app.profiles.selected && app.worker.is_none()
                    )
                ]
                .spacing(20)
                .align_y(Alignment::Center),
            )
            .padding(18)
            .style(p::panel),
        );
    }
    items = items
        .push(field("Profile name", &app.form.profile.title, Field::Title))
        .push(
            row![
                action("Save profile", Message::Save, app.store.is_some()),
                action(
                    "New profile",
                    Message::NewProfile,
                    app.store.is_some() && app.worker.is_none() && app.profiles.items.len() < 30
                )
            ]
            .spacing(12),
        );
    if !app.notice.is_empty() {
        items = items.push(text(&app.notice).size(13).color(if app.notice_error {
            p::ERROR
        } else {
            p::MUTED
        }));
    }
    effects::frame(
        container(
            column![
                super::widgets::heading("Profiles", Icon::People),
                container(scrollable(items).style(p::scrollbar))
                    .padding(24)
                    .height(Fill)
            ]
            .height(Fill),
        )
        .height(Fill)
        .style(p::panel),
    )
}

fn activity(app: &App) -> Element<'_, Message> {
    let mut entries = column![].spacing(0);
    for entry in &app.logs {
        entries = entries
            .push(
                container(
                    row![muted(&entry.time).width(60), text(&entry.message).size(14)].spacing(16),
                )
                .padding([14, 12])
                .width(Fill)
                .style(if entry.highlight {
                    p::activity_highlight
                } else {
                    p::plain
                }),
            )
            .push(rule::horizontal(1));
    }
    effects::frame(
        container(
            column![
                row![
                    super::widgets::heading("Activity", Icon::Scroll),
                    space().width(Fill),
                    action("Clear", Message::ClearLog, true)
                ]
                .align_y(Alignment::Center),
                scrollable(entries).style(p::scrollbar).height(Fill)
            ]
            .spacing(20),
        )
        .padding(20)
        .height(Fill)
        .style(p::panel),
    )
}
