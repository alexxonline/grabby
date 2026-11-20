# Grabby (Rust)

A native Windows meeting recorder and transcription app, written as one Rust desktop app. The UI
uses [Rune](https://github.com/anomalyco/rune) with its `rune-kit` components.

Features:

- records your microphone and system audio (WASAPI loopback) to a crash-safe raw PCM capture,
  then mixes it and exports MP3 with `ffmpeg` (WAV if `ffmpeg` is missing)
- shows live captions from ElevenLabs realtime STT (`scribe_v3_live`) while recording
- transcribes saved recordings with ElevenLabs Scribe v3 with speaker diarization
- edits transcripts (speaker renaming), notes per recording, full-text search (SQLite FTS5)
- auto-delete audio and auto title (OpenRouter) settings
- Auto / Light / Dark appearance (Settings); Auto follows the Windows app mode
- text sync across devices (wire contract in [`../SYNC.md`](../SYNC.md)), including the local mock server

## ElevenLabs behaviour

Requests match what the official ElevenLabs SDK sends:

- **Batch:** `POST https://api.elevenlabs.io/v1/speech-to-text`, multipart, header `xi-api-key`,
  fields `model_id=scribe_v3`, `tag_audio_events=false`, `diarize=true`, plus `file`. 240 s
  timeout and two retries on 408/409/429/5xx, as the SDK does. Word-level results are grouped into
  `Speaker N: ...` lines.
- **Realtime:** WebSocket `wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v3_live&audio_format=pcm_16000&commit_strategy=vad&include_timestamps=false`,
  one connection per source ("You" = mic, "Others" = system audio), 16 kHz mono PCM sent as
  `input_audio_chunk` messages in chunks of at least 200 ms.

## Build

Rust is pinned by `rust-toolchain.toml` (1.99.1, Rune's toolchain; rustup installs it
automatically). The MSVC toolchain needs the Visual Studio C++ build tools.

```powershell
cd grabby-rust
cargo build --release
```

The app is `target\release\grabby.exe`. Release builds are a GUI app with no console window.
Debug builds keep the console for logs:

```powershell
cargo run
```

Tests (service logic, sync round trip with conflicts, PCM mixing, parsing helpers):

```powershell
cargo test
```

Rune is pre-release and only on git, so `Cargo.toml` pins a `dev` revision. Rune requires apps to
carry its vendored patches (`wgpu-hal`, `winit`, `parley_engine`, `accesskit_*`); the
`[patch.crates-io]` section takes them from the same git revision.

## Configuration

Data lives in `%LOCALAPPDATA%\Grabby` by default:

- `grabby.db` — SQLite database
- `recordings\` — `.mp3` / `.wav` files and in-progress `.capture-*` folders
- `configs.dat` — API keys, encrypted with Windows DPAPI

Set `GRABBY_DATA_DIR` to use another folder:

```powershell
$env:GRABBY_DATA_DIR = "D:\grabby-data"
```

### API keys

Enter keys in **Settings > API keys**:

| Key | Used for |
| --- | --- |
| ElevenLabs (`ELEVENLABS_API_KEY`) | Required: live captions and transcription |
| OpenRouter (`OPENROUTER_API_KEY`) | Optional: Auto title |
| Text sync (`GRABBY_SYNC_API_KEY`) | Optional: Bearer token for your sync endpoint (also settable in Settings > Text sync) |

Each key is encrypted with `CryptProtectData` (DPAPI) for the current Windows user, with entropy
that ties the blob to its key name, and stored base64-encoded in `configs.dat`. Only that Windows
user on that PC can decrypt it; the file is useless if copied elsewhere. Saved keys are never
displayed again; typing a new one replaces it. Keys are decrypted only when a request needs them,
and the input buffers are wiped after saving.

Keys are never read from `.env`. When the app starts, any of these keys found in
`<data dir>\.env` are encrypted into `configs.dat` and their lines in `.env` are replaced with a
comment. A process environment variable with the same name still works as a fallback (useful for
CI); a key saved in Settings takes priority.

`.env` (see `.env.example`) holds only non-secret settings. `ffmpeg` must be on `PATH` for MP3
export.

## Layout

| Module | Responsibility |
| --- | --- |
| `src/core.rs` | Service layer called directly by the UI |
| `src/db.rs` | SQLite schema and queries |
| `src/sync.rs` | Text sync, conflicts, local mock receiver |
| `src/recorder.rs`, `src/audio.rs` | Capture, resampling, mixing, MP3/WAV export |
| `src/realtime.rs` | ElevenLabs realtime captions |
| `src/transcription.rs` | Batch transcription |
| `src/titles.rs` | OpenRouter auto titles |
| `src/secrets.rs`, `src/config.rs` | DPAPI key storage, data directory |
| `src/ui/*` | Rune pages and components |

Headless snapshots of each page (debug builds), useful for checking the UI:

```powershell
cargo run -- --snap recorder.png --scene recorder   # also: recordings, search, settings
```

## Design notes

- No HTTP server: the UI calls the service layer directly. The sync mock transport calls the
  receiver in-process.
- Delete asks for confirmation inline; Download opens a native Save dialog.
- Notes autosave (600 ms) and save when you switch tab, page or close the window.
- Audio capture uses `cpal`. WASAPI glitches (xruns) and default-device switches are logged
  instead of stopping the recording, and silence is written to the system track while nothing
  plays, so microphone and system audio stay aligned.
- The resampler uses linear interpolation.
