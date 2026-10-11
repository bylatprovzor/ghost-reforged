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
mod view;
mod widgets;

use clap::Parser;
use iced::Task;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "GHost Reforged — Warcraft III hosting profile launcher")]
struct Args {
    /// Use a separate profile directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Preselect a map without modifying the file.
    #[arg(long)]
    map: Option<PathBuf>,
    /// Start hosting with the selected profile when the launcher opens.
    #[arg(long)]
    host: bool,
}

fn main() -> iced::Result {
    let args = Args::parse();
    let directory = args.data_dir.unwrap_or_else(|| {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        storage::Store::default_directory(&root)
    });
    iced::application(
        move || {
            let mut state = app::App::new(&directory, args.map.clone());
            let mut task = state.update(app::Message::PollProcess);
            if args.host {
                task = Task::batch([task, Task::done(app::Message::Host)]);
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
