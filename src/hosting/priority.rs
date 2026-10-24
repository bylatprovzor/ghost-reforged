//! Preserve startup progress while keeping a confirmed lobby behind desktop work.
use crate::{background::Background, engine::Engine};

pub(super) fn connecting(game: &mut Background) -> Result<(), String> {
    // Battle.net's foreground-priority helpers can otherwise starve startup.
    // Release only our own throttle; preserve externally chosen priorities.
    game.background_priority(false)
}

pub(super) fn observe(
    game: &mut Background,
    engine: &Engine,
    identity_ready: bool,
    now: u64,
) -> Result<(), String> {
    game.background_priority(super::render::pause_browser(engine, identity_ready, now))
}

#[cfg(all(test, windows))]
#[path = "priority_tests.rs"]
mod tests;
