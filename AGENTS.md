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
- Python 3.13 is in use — `audioop` was removed; install `audioop-lts` for pydub compatibility
- pydub requires `ffmpeg` on PATH for MP3 export; without it, recordings fall back to WAV
- WASAPI loopback captures system audio (other participants); a separate mic stream captures your voice; both are mixed together

## Frontend Details
- Routing via `preact-router`
- API client in `src/api.ts` — all backend calls go through `http://localhost:8000/api`
- Pages in `src/pages/` — `RecorderPage.tsx` (record controls) and `RecordingsPage.tsx` (list/rename/delete/play)
- Reusable components in `src/components/` — e.g. `AudioPlayer.tsx` (seekable audio player)
- Global styles in `src/app.css`; CSS variables (theme) defined in `src/index.css`
- Audio playback uses the HTML `<audio>` element with the backend's `/api/recordings/{filename}/download` endpoint as `src`; no extra dependencies needed

## Dependencies & Gotchas
- `pyaudiowpatch` is the Windows WASAPI fork of PyAudio — required for loopback recording
- `audioop-lts` must be installed alongside `pydub` on Python 3.13+
- Frontend uses `preact-router` (not `react-router-dom`)
- CORS is wide open (`allow_origins=["*"]`) for local dev