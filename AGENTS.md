# AGENTS.md

## Stack

* Single Rust desktop app in `grabby-rust/` (Windows only)
* Toolchain pinned by `grabby-rust/rust-toolchain.toml` (Rune's toolchain); rustup installs it automatically
* UI: [Rune](https://github.com/anomalyco/rune) + `rune-kit`, pinned to a git revision in `Cargo.toml`
* Database: SQLite + FTS5 (`rusqlite`, bundled)
* Audio capture: `cpal` (WASAPI loopback for system audio); playback via `rodio`
* HTTP/WebSocket: `reqwest` and `tokio-tungstenite` (ElevenLabs, OpenRouter, sync endpoint)
* Secrets: Windows DPAPI via `windows-sys` (`src/secrets.rs`)

## Run

From `grabby-rust/`:

* `cargo run`: debug build with a console for logs
* `cargo build --release`: GUI build at `target\release\grabby.exe`
* `cargo test`: unit tests (service logic, sync round trip, PCM mixing, parsers)
* `cargo clippy` and `cargo fmt`: both are installed by the toolchain file
* `cargo run -- --snap out.png --scene recorder`: headless UI snapshot (also `recordings`, `search`, `settings`)

## Layout

* `src/core.rs`: service layer called directly by the UI (no HTTP server)
* `src/db.rs`: SQLite schema and queries
* `src/sync.rs`: text sync; wire contract in `SYNC.md`
* `src/recorder.rs`, `src/audio.rs`: capture, mixing, MP3/WAV export
* `src/realtime.rs`, `src/transcription.rs`, `src/titles.rs`: ElevenLabs live captions, batch transcription, OpenRouter titles
* `src/secrets.rs`, `src/config.rs`: DPAPI key storage, data directory and settings
* `src/ui/*`: Rune pages and components

## Important

* Use the existing architecture and dependencies; inspect the code before adding new ones.
* Do not bump the Rune revision, the `[patch.crates-io]` entries or the toolchain casually; they must stay in sync with each other.
* `ffmpeg` must be on `PATH` for MP3 export (WAV is the fallback).
* API keys are stored encrypted in `configs.dat` (DPAPI), never in `.env`. `.env` holds only non-secret settings; never commit secrets.
* Data lives in `%LOCALAPPDATA%\Grabby` unless `GRABBY_DATA_DIR` is set.
