# 5. Persistence, data directory and UI threading

Status: accepted

## Decisions

- **SQLite per thread.** Every thread owns its connection (WAL, 5 s busy timeout). A failed initialisation closes its connection.
- **Data directory.** `GRABBY_DATA_DIR`, else `%LOCALAPPDATA%\Grabby`. Tests always use a temporary directory, so they never touch real data.
- **No HTTP layer.** The UI calls the service layer in `core.rs` directly. Slow work (capture start/stop, sync, transcription requests) runs on worker threads and returns to the UI thread through `AppHandle::update`.
- **Window icon.** The icon is the exe resource embedded by `build.rs` (winresource uses id 1), selected with `set_app_icon_resource(1)`.
