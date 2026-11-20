//! Grabby: a local meeting recorder and transcription app for Windows.
//!
//! Records microphone + system audio (WASAPI loopback), transcribes with ElevenLabs Scribe v3,
//! stores transcripts in SQLite FTS5 and renders its UI with Rune.

// Release builds are a GUI app without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod config;
mod core;
mod db;
mod realtime;
mod recorder;
mod secrets;
mod speakers;
mod sync;
mod timefmt;
mod titles;
mod transcription;
mod ui;

fn main() {
    config::load_env();
    // Keys left in plain text in `.env` move into the DPAPI-encrypted configs.dat.
    secrets::migrate_from_env_file();
    core::ensure_database();
    core::start_background_sync();
    ui::run();
}
