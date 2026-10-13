#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod art;
#[cfg(all(test, windows))]
mod audio_tests;
mod backdrop;
mod backdrop_view;
mod chrome;
mod desktop;
mod effects;
mod frame_art;
mod hosting;
mod hosting_view;
mod journal;
mod launch_args;
mod map_name;
mod maps;
mod motion;
mod options_view;
mod palette;
mod preview;
mod preview_view;
mod process;
mod profile;
mod select;
mod storage;
#[cfg(test)]
mod tests;
mod tray;
#[cfg(test)]
mod ui_tests;
mod update_ui;
mod updates;
mod view;
mod widgets;

use clap::Parser;
use ghost_reforged::console::{self, Args};
use iced::Task;

fn main() -> iced::Result {
    if ghost_reforged::background::render_guardian_entry() {
        return Ok(());
    }
    if std::env::args().len() > 1 {
        console::attach_console();
    }
    let args = Args::parse();
    if args.no_gui || args.command.is_some() {
        let result = console::run(args);
        if let Err(error) = &result {
            eprintln!("{error}");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    let directory = console::directory(&args);
    iced::application(
        move || {
            let mut state = app::App::new(&directory, None);
            let configured = state.apply_launch_args(&args);
            let ready = configured.is_ok();
            state.notify(configured, "");
            let task = Task::batch([
                state.update(app::Message::PollProcess),
                Task::perform(updates::check(), app::Message::UpdateChecked),
            ]);
            if args.host && ready {
                let result = state.start_hosting();
                state.notify(result, "");
            }
            (state, task)
        },
        app::App::update,
        view::view,
    )
    .title("GHost Reforged")
    .theme(palette::theme)
    .default_font(iced::Font::with_name("Segoe UI"))
    .window(iced::window::Settings {
        icon: iced::window::icon::from_rgba(
            include_bytes!("../../design/ghost.rgba").to_vec(),
            64,
            64,
        )
        .ok(),
        size: iced::Size::new(1180.0, 856.0),
        decorations: false,
        min_size: Some(iced::Size::new(1000.0, 720.0)),
        position: iced::window::Position::Centered,
        exit_on_close_request: false,
        ..Default::default()
    })
    .subscription(app::App::subscription)
    .run()
}
