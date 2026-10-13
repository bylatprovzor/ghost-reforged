use clap::Parser;
fn main() -> std::process::ExitCode {
    if ghost_reforged::background::render_guardian_entry() {
        return std::process::ExitCode::SUCCESS;
    }
    let args = ghost_reforged::console::Args::parse();
    match ghost_reforged::console::run(args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
