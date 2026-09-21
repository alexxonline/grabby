# AGENTS.md

## Stack

* Backend: Python 3.13 + FastAPI in `grabby-backend/`
* Backend virtualenv: `grabby-backend/.venv`
* Frontend: Preact + TypeScript + Vite in `grabby-front/`
* Database: SQLite + FTS5
* Windows audio capture: `pyaudiowpatch` / WASAPI loopback

## Run

Backend:
`.venv\Scripts\uvicorn app.main:app --reload --port 8000`

Frontend:
`npm run dev`

## Important

* Use the existing architecture and dependencies; inspect the code before introducing new ones.
* Frontend routing uses `preact-router`, not `react-router-dom`.
* TypeScript has `verbatimModuleSyntax`; use `import type` for types.
* Python 3.13 requires `audioop-lts` for `pydub`.
* `pydub` requires `ffmpeg` on PATH for MP3 export.
* Environment variables belong in `grabby-backend/.env`; never commit secrets.
