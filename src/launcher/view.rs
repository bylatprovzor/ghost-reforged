use super::{
    app::{App, Field, Message, Page},
    palette as p,
    process::GameState,
    profile::ProfileChoice,
};
use iced::{
    Alignment, Border, Color, Element, Fill, Length,
    widget::{
        button, column, container, pick_list, row, rule, scrollable, space, svg, text, text_input,
        tooltip,
    },
};

pub(super) fn muted<'a>(value: impl Into<String>) -> iced::widget::Text<'a> {
    text(value.into()).size(13).color(p::MUTED)
}

pub(super) fn field<'a>(label: &'a str, value: &'a str, kind: Field) -> Element<'a, Message> {
    field_enabled(label, value, kind, true)
}

fn field_enabled<'a>(
    label: &'a str,
    value: &'a str,
    kind: Field,
    enabled: bool,
) -> Element<'a, Message> {
    let input = text_input(label, value).padding(11).size(15);
    let input = if enabled {
        input.on_input(move |value| Message::Edit(kind, value))
    } else {
        input
    };
    column![text(label).size(13).color(p::MUTED), input]
        .spacing(7)
        .into()
}

pub(super) fn action(
    label: &'static str,
    message: Message,
    enabled: bool,
) -> Element<'static, Message> {
    button(text(label).size(13))
        .padding([10, 14])
        .style(p::secondary)
        .on_press_maybe(enabled.then_some(message))
        .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let ghost = svg(svg::Handle::from_memory(include_bytes!(
        "../../design/ghost.svg"
    )))
    .width(25)
    .height(29);
    let top = container(
        row![ghost, text("GHost Reforged").size(22)]
            .spacing(13)
            .align_y(Alignment::Center),
    )
    .padding([17, 25]);
    let mut navigation = column![space().height(8)].spacing(8);
    for (page, label) in [
        (Page::Hosting, "Hosting"),
        (Page::Profiles, "Profiles"),
        (Page::Options, "Options"),
        (Page::Activity, "Activity"),
    ] {
        navigation = navigation.push(
            button(text(label).size(14))
                .padding([13, 16])
                .width(Fill)
                .style(if app.page == page {
                    p::primary
                } else {
                    p::secondary
                })
                .on_press(Message::Page(page)),
        );
    }
    let color = match app.game {
        GameState::Stopped => Color::from_rgb8(223, 94, 94),
        GameState::Background | GameState::Visible => p::ACCENT,
        GameState::Multiple => Color::from_rgb8(216, 185, 120),
        GameState::Checking | GameState::Unavailable => p::MUTED,
    };
    let led = container(space())
        .width(8)
        .height(8)
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    let status = tooltip(
        container(
            row![led, text(app.game.label()).size(12).color(p::MUTED)]
                .spacing(9)
                .align_y(Alignment::Center),
        )
        .padding(12),
        container(text(app.game.tooltip()).size(12))
            .padding(9)
            .style(p::panel),
        tooltip::Position::Right,
    );
    navigation = navigation.push(space().height(Fill)).push(status);
    let sidebar = container(navigation)
        .width(160)
        .height(Fill)
        .padding([18, 14]);
    let body = match app.page {
        Page::Hosting => hosting(app),
        Page::Profiles => profiles(app),
        Page::Activity => activity(app),
        Page::Options => super::options_view::view(app),
    };
    let content = row![
        sidebar,
        rule::vertical(1),
        container(body).padding(28).width(Fill)
    ]
    .height(Fill);
    let base = container(column![top, rule::horizontal(1), content])
        .width(Fill)
        .height(Fill)
        .style(p::plain)
        .into();
    if app.preview.expanded {
        super::preview_view::modal(app, base)
    } else {
        base
    }
}

fn hosting(app: &App) -> Element<'_, Message> {
    let idle = app.worker.is_none();
    let editable = idle && !app.picking && app.store.is_some();
    let choices: Vec<_> = app
        .profiles
        .items
        .iter()
        .enumerate()
        .map(|(index, profile)| ProfileChoice {
            index,
            title: profile.title.clone(),
        })
        .collect();
    let selected = choices.get(app.profiles.selected).cloned();
    let selector: Element<'_, Message> = if editable {
        pick_list(choices, selected, |choice| {
            Message::SelectProfile(choice.index)
        })
        .padding(10)
        .width(Fill)
        .into()
    } else {
        container(text(&app.form.profile.title).size(14))
            .padding(10)
            .width(Fill)
            .style(p::panel)
            .into()
    };
    let map =
        container(column![muted("MAP"), text(app.form.profile.map_label()).size(14)].spacing(8))
            .width(Fill)
            .padding(14)
            .style(p::panel);
    let threshold = pick_list(
        ["Full", "Minimum players"],
        Some(if app.form.profile.require_full {
            "Full"
        } else {
            "Minimum players"
        }),
        |choice| Message::RequireFull(choice == "Full"),
    )
    .padding(11)
    .width(Fill);
    let start_rule: Element<'_, Message> = if app.form.profile.require_full {
        column![muted("Auto start"), threshold].spacing(7).into()
    } else {
        column![
            muted("Auto start"),
            threshold,
            field("Minimum players", &app.form.players, Field::Players)
        ]
        .spacing(7)
        .into()
    };
    let controls = if idle {
        row![
            button(text("Host").size(14))
                .padding([13, 19])
                .style(p::primary)
                .on_press_maybe(editable.then_some(Message::Host))
        ]
    } else {
        let stopping = app.hosting_status == "Stopping…";
        row![
            button(text("Force start").size(14))
                .padding([13, 19])
                .style(p::primary)
                .on_press_maybe(
                    (!stopping && app.lobby.as_ref().is_some_and(|l| l.can_start))
                        .then_some(Message::Start)
                ),
            action("Stop Hosting", Message::Stop, !stopping),
        ]
    }
    .spacing(10);
    let mut inputs = column![
        text("Auto Host").size(32),
        column![
            muted("Profile"),
            row![
                selector,
                action(
                    "New",
                    Message::NewProfile,
                    editable && app.profiles.items.len() < 30
                )
            ]
            .spacing(8)
        ]
        .spacing(7),
        rule::horizontal(1),
        row![map, action("Browse", Message::Browse, idle && !app.picking)]
            .spacing(10)
            .align_y(Alignment::Center),
        field("Host name", &app.form.profile.bot_name, Field::Bot),
        row![
            field_enabled(
                "Game name",
                &app.form.profile.lobby_pattern,
                Field::Pattern,
                idle
            ),
            column![
                space().height(20),
                action(
                    "Use map name",
                    Message::MapName,
                    editable && app.form.profile.map.is_some()
                )
            ],
        ]
        .spacing(8),
        row![
            field_enabled("Next number", &app.form.number, Field::Number, idle),
            start_rule,
        ]
        .spacing(12),
    ]
    .spacing(14);
    inputs = inputs.push(controls);
    if !app.notice.is_empty() {
        inputs = inputs.push(text(&app.notice).size(13).color(if app.notice_error {
            p::ERROR
        } else {
            p::MUTED
        }));
    }
    row![
        scrollable(inputs).width(Length::FillPortion(3)),
        column![preview(app), super::preview_view::thumbnail(app)]
            .spacing(16)
            .height(Fill)
            .width(Length::FillPortion(2))
    ]
    .spacing(28)
    .into()
}

fn preview(app: &App) -> Element<'_, Message> {
    if let Some(lobby) = &app.lobby {
        return live_lobby(app, lobby);
    }
    if app.worker.is_some() {
        return column![
            text("Lobby").size(22),
            container(muted(&app.hosting_status))
                .padding(20)
                .width(Fill)
                .style(p::panel),
        ]
        .spacing(17)
        .height(Fill)
        .into();
    }
    let profile = &app.form.profile;
    let number = app.form.number.parse().unwrap_or(1);
    let capacity = app.form.players.parse::<u8>().unwrap_or(4).clamp(1, 23);
    let mut players = column![].spacing(4);
    for n in 0..capacity {
        players = players.push(
            container(
                row![
                    text(format!("{:02}", n + 1))
                        .size(12)
                        .color(p::MUTED)
                        .width(27),
                    text("Open slot").size(14).color(p::MUTED),
                    space().width(Fill),
                    muted("—")
                ]
                .align_y(Alignment::Center),
            )
            .padding([15, 0]),
        );
    }
    let lobby = container(
        column![
            row![
                text(profile.lobby_name(number)).size(20),
                space().width(Fill),
                muted(format!("0 / {capacity}"))
            ]
            .align_y(Alignment::Center),
            rule::horizontal(1),
            scrollable(players).height(Fill),
            rule::horizontal(1),
            row![
                text(&profile.bot_name).size(15),
                space().width(Fill),
                muted("Observer")
            ]
            .padding([10, 0])
        ]
        .spacing(16)
        .height(Fill),
    )
    .padding(20)
    .height(Fill)
    .style(p::panel);
    column![
        text("Lobby preview").size(22),
        space().height(4),
        lobby,
        container(muted(&app.hosting_status)).padding([10, 2])
    ]
    .spacing(17)
    .height(Fill)
    .into()
}

fn live_lobby<'a>(
    app: &'a App,
    lobby: &'a ghost_reforged::hosting::Snapshot,
) -> Element<'a, Message> {
    let mut slots = column![].spacing(4);
    for slot in &lobby.slots {
        let label = if slot.own {
            slot.name.clone()
        } else if slot.status == 0 {
            "Open slot".into()
        } else if slot.status == 1 {
            "Closed slot".into()
        } else if slot.computer {
            "Computer".into()
        } else {
            if slot.name.is_empty() {
                format!("Player {}", slot.number + 1)
            } else {
                slot.name.clone()
            }
        };
        let mut item = row![
            muted(format!("{:02}", slot.number + 1)).width(27),
            text(label).size(14),
            space().width(Fill)
        ]
        .align_y(Alignment::Center);
        if slot.observer {
            item = item.push(muted("Observer"));
        }
        if !slot.own && matches!(slot.status, 0 | 1) {
            item = item.push(action(
                if slot.status == 0 { "Close" } else { "Open" },
                Message::Slot(slot.number, slot.status == 1),
                app.worker.is_some() && lobby.can_edit_slots,
            ));
        }
        slots = slots.push(container(item).padding([8, 0]));
    }
    let capacity = lobby.player_capacity();
    let panel = container(
        column![
            row![
                text(&lobby.name).size(20),
                space().width(Fill),
                muted(format!("{} / {capacity}", lobby.humans))
            ]
            .align_y(Alignment::Center),
            rule::horizontal(1),
            scrollable(container(slots).padding(iced::Padding {
                right: 12.0,
                ..Default::default()
            }))
            .height(Fill)
        ]
        .spacing(16)
        .height(Fill),
    )
    .padding(20)
    .height(Fill)
    .style(p::panel);
    column![text("Lobby").size(22), panel, muted(&app.hosting_status)]
        .spacing(17)
        .height(Fill)
        .into()
}

fn profiles(app: &App) -> Element<'_, Message> {
    let mut items = column![text("Your profiles.").size(32), space().height(8)].spacing(16);
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
    scrollable(items).into()
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
    column![
        row![
            text("Activity.").size(32),
            space().width(Fill),
            action("Clear", Message::ClearLog, true)
        ]
        .align_y(Alignment::Center),
        scrollable(entries).height(Fill)
    ]
    .spacing(20)
    .into()
}
