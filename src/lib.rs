pub mod commands;
pub mod config;
pub mod diagnostics;
pub mod discovery;
pub mod engine;
pub mod hosting;
pub mod protocol;
pub mod recovery;
pub mod transport;
pub mod welcome;
pub mod window_capture;

pub mod background;
mod host_identity;
#[cfg(test)]
mod tests;

#[cfg(windows)]
mod background_audio;
#[cfg(windows)]
mod background_launcher;
#[cfg(windows)]
mod background_window;
