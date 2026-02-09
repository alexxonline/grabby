## Foundations
Use Python with fastapi for the backend 
Use grabby-backend/.venv virtual env folder
Use Preact for the frontend

## Project Structure
- `grabby-backend/` — FastAPI backend (Python). Run with `.venv\Scripts\uvicorn app.main:app --reload --port 8000`
- `grabby-front/` — Preact frontend (TypeScript + Vite). Run with `npm run dev`
- `grabby-backend/recordings/` — Where audio recordings are saved
- `research/` — Research notes

## Backend Details
- Entry point: `grabby-backend/app/main.py` (FastAPI app)
- Recorder service: `grabby-backend/app/recorder.py` (singleton `MeetingRecorder`)
- Database: `grabby-backend/app/database.py` — SQLite with FTS5 for transcription storage & search
- Transcription: `grabby-backend/app/transcription.py` — ElevenLabs Scribe v2 with speaker diarization
- DB file: `grabby-backend/grabby.db` (auto-created on startup)
- Python 3.13 is in use — `audioop` was removed; install `audioop-lts` for pydub compatibility
- pydub requires `ffmpeg` on PATH for MP3 export; without it, recordings fall back to WAV
- WASAPI loopback captures system audio (other participants); a separate mic stream captures your voice; both are mixed together
- Requires `ELEVENLABS_API_KEY` env var — set in `grabby-backend/.env` (see `.env.example`)

## Frontend Details
- Routing via `preact-router`
- API client in `src/api.ts` — all backend calls go through `http://localhost:8000/api`
- Pages in `src/pages/` — `RecorderPage.tsx` (record controls), `RecordingsPage.tsx` (list/rename/delete/play), and `TranscriptionPage.tsx` (view/edit transcript with speaker rename)
- Reusable components in `src/components/` — e.g. `AudioPlayer.tsx` (seekable audio player)
- Global styles in `src/app.css`; CSS variables (theme) defined in `src/index.css`
- Audio playback uses the HTML `<audio>` element with the backend's `/api/recordings/{filename}/download` endpoint as `src`; no extra dependencies needed
- Route params: `preact-router` passes URL params as component props (e.g. `/transcription/:filename` → `filename` prop)
- When destructuring `path` from router props but not using it, prefix with underscore (`_path`) to avoid TS unused-variable errors

## Transcription Feature
- Transcription page at `/transcription/:filename` — shows full transcript with speaker rename UI
- Speaker labels (`Speaker A:`, `Speaker B:`, etc.) are extracted via regex and shown as editable chips at the top
- Clicking a chip opens an inline input; pressing Enter or blurring confirms the rename across the entire transcript text
- Renames are persisted via `PUT /api/recordings/{filename}/transcription` → `db.update_transcription_text()`
- The "📝 Transcript" button on RecordingsPage navigates to this page (no longer expands inline)
- Backend `database.py` has `update_transcription_text()` for updating transcript text in SQLite (triggers keep FTS5 in sync)

## Dependencies & Gotchas
- `pyaudiowpatch` is the Windows WASAPI fork of PyAudio — required for loopback recording
- `audioop-lts` must be installed alongside `pydub` on Python 3.13+
- `elevenlabs` SDK + `python-dotenv` for transcription
- Frontend uses `preact-router` (not `react-router-dom`)
- CORS is wide open (`allow_origins=["*"]`) for local dev