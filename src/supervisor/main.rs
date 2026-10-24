#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::Parser;
mod failure;
#[derive(Parser)]
#[command(about = "GHost Reforged resident supervisor", version)]
struct Args {
    #[arg(long)]
    data_dir: std::path::PathBuf,
    #[arg(long, hide = true)]
    controller: bool,
}

fn main() -> std::process::ExitCode {
    if ghost_reforged::background::render_guardian_entry() {
        return std::process::ExitCode::SUCCESS;
    }
    let args = Args::parse();
    let directory = args.data_dir.clone();
    let result = if args.controller {
        ghost_reforged::console::run(ghost_reforged::console::Args {
            data_dir: Some(args.data_dir),
            profile: None,
            map: None,
            host: false,
            resume: true,
            headless: false,
            normal: false,
            no_gui: true,
            command: None,
        })
    } else {
        ghost_reforged::supervisor::run(&args.data_dir)
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let _ = failure::record(&directory, args.controller, &error);
            std::process::ExitCode::FAILURE
        }
    }
}
