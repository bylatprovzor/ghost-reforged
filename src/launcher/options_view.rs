use super::{
    app::{App, Field, Message},
    palette as p,
    profile::GameSpeed,
    view::{field, muted},
};
use ghost_reforged::commands::ChatCommand;
use iced::{
    Element, Fill,
    widget::{column, container, pick_list, row, rule, scrollable, text, toggler},
};

pub fn view<'a>(app: &'a App) -> Element<'a, Message> {
    let profile = &app.form.profile;
    let editable = app.store.is_some();
    let mut commands: iced::widget::Column<'a, Message> =
        column![text("Chat commands").size(20)].spacing(18);
    for command in ChatCommand::ALL {
        let toggle = toggler(profile.commands.enabled(command))
            .label(command.name())
            .size(21)
            .text_size(14);
        let toggle = if editable {
            toggle.on_toggle(move |enabled| Message::Command(command, enabled))
        } else {
            toggle
        };
        commands = commands.push(column![toggle, muted(command.description())].spacing(6));
    }
    let mut settings: iced::widget::Column<'a, Message> = column![
        text("Options").size(32),
        muted(format!("Profile · {}", profile.title)),
        rule::horizontal(1),
        text("Welcome message").size(20),
        toggler(profile.welcome.enabled)
            .label("Welcome joining players")
            .size(21)
            .text_size(14)
            .on_toggle(Message::Welcome),
        field("Message", &profile.welcome.message, Field::Welcome),
        muted("Use #{playername} for the joining player's name."),
        rule::horizontal(1),
        commands,
        rule::horizontal(1),
        text("Hosting").size(20),
        toggler(profile.rehost)
            .label("Create next lobby after countdown")
            .size(21)
            .text_size(14)
            .on_toggle(Message::Rehost),
        if app.worker.is_none() {
            column![
                toggler(profile.private_game)
                    .label("Private lobby")
                    .size(21)
                    .text_size(14)
                    .on_toggle(Message::Private),
                column![
                    muted("Game speed"),
                    pick_list(GameSpeed::ALL, Some(profile.speed), Message::Speed)
                        .padding(10)
                        .width(Fill)
                ]
                .spacing(7),
            ]
            .spacing(16)
        } else {
            column![
                muted(format!(
                    "{} lobby · {} speed",
                    if profile.private_game {
                        "Private"
                    } else {
                        "Public"
                    },
                    profile.speed
                )),
                muted("Map, game name, visibility and speed are set when the lobby is created.")
            ]
            .spacing(7)
        },
        row![
            field(
                "Rehost delay (s)",
                &app.form.rehost_delay,
                Field::RehostDelay
            ),
            field(
                "Empty lobby refresh (s, 0 = off)",
                &app.form.empty_refresh,
                Field::EmptyRefresh
            )
        ]
        .spacing(12),
    ]
    .spacing(16)
    .max_width(760);

    if !app.notice.is_empty() {
        settings = settings.push(text(&app.notice).size(13).color(if app.notice_error {
            p::ERROR
        } else {
            p::MUTED
        }));
    }
    scrollable(container(settings).padding(iced::Padding {
        right: 16.0,
        ..Default::default()
    }))
    .into()
}
