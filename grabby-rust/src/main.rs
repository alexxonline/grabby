
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
    secrets::migrate_from_env_file();
    core::ensure_database();
    core::start_background_sync();
    ui::run();
}
