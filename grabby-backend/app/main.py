import os
from datetime import datetime
from pathlib import Path

from dotenv import load_dotenv
from fastapi import BackgroundTasks, FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from .recorder import recorder, RECORDINGS_DIR
from . import database as db
from .transcription import transcribe_file
from .titles import suggest_title, MODEL

load_dotenv(Path(__file__).resolve().parent.parent / ".env")

app = FastAPI(title="Grabby – Meeting Recorder")

# Initialise database on startup
db.init_db()

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)


# ----- Schemas -----

class SettingsUpdate(BaseModel):
    auto_delete_audio: bool
    auto_title: bool


class StartRequest(BaseModel):
    name: str | None = None


class RecordingStatus(BaseModel):
    recording: bool
    filename: str | None = None
    started_at: str | None = None
    live: dict | None = None


class RecordingInfo(BaseModel):
    filename: str
    name: str
    size_bytes: int
    audio_available: bool = True
    processing_error: str | None = None
    processing: bool = False
    created_at: str


class RenameRequest(BaseModel):
    new_name: str


class TranscriptionOut(BaseModel):
    filename: str
    text: str
    language: str | None = None
    status: str = "completed"


class TranscriptionStatus(BaseModel):
    filename: str
    title: str | None = None
    status: str  # "pending" | "completed" | "error"
    text: str | None = None
    language: str | None = None
    error: str | None = None


class SearchResult(BaseModel):
    filename: str
    title: str | None = None
    snippet: str
    language: str | None = None


class UpdateTranscriptionRequest(BaseModel):
    text: str


# Track in-flight transcription jobs
_transcribing: dict[str, str] = {}  # filename -> status ("pending"|"error")
_transcribe_errors: dict[str, str] = {}  # filename -> error message


# ----- Endpoints -----

@app.get("/api/settings")
def get_settings():
    return {**db.get_settings(), "openrouter_configured": bool(os.getenv("OPENROUTER_API_KEY", "").strip()),
            "title_model": MODEL}


@app.put("/api/settings")
def update_settings(body: SettingsUpdate):
    db.save_settings(body.model_dump())
    return get_settings()


def _recording_path(filename: str) -> Path:
    if Path(filename).name != filename or any(c in filename for c in '/\\:') or filename in (".", ".."):
        raise HTTPException(status_code=400, detail="Invalid recording filename")
    return RECORDINGS_DIR / filename


def _recording_info(filename: str) -> RecordingInfo:
    filepath = _recording_path(filename)
    try:
        stat = filepath.stat() if filepath.is_file() else None
    except FileNotFoundError:
        # Automatic cleanup may finish while the recordings list is loading.
        stat = None
    transcript = db.get_transcription(filename)
    if stat is None and transcript is None:
        raise HTTPException(status_code=404, detail="Recording not found")
    created_at = datetime.fromtimestamp(stat.st_mtime).isoformat() if stat else transcript["created_at"]
    db.ensure_metadata(filename, created_at)
    metadata = db.get_metadata(filename)
    return RecordingInfo(filename=filename, name=metadata.get("title") or filepath.stem,
                         size_bytes=stat.st_size if stat else 0, audio_available=stat is not None,
                         created_at=metadata["created_at"], processing_error=metadata.get("processing_error"),
                         processing=_transcribing.get(filename) == "pending")


def _require_idle(filename: str):
    if _transcribing.get(filename) == "pending" or (recorder.is_recording and Path(filename).stem == recorder.current_file):
        raise HTTPException(status_code=409, detail="Wait until recording and processing finish.")


@app.get("/api/status", response_model=RecordingStatus)
def get_status():
    return RecordingStatus(
        recording=recorder.is_recording,
        filename=recorder.current_file,
        started_at=recorder.started_at.isoformat() if recorder.started_at else None,
        live=recorder.live.snapshot() if recorder.is_recording and recorder.live else None,
    )


@app.post("/api/start", response_model=RecordingStatus)
def start_recording(body: StartRequest | None = None):
    try:
        name = body.name if body else None
        recorder.start(filename=name)
    except RuntimeError as e:
        raise HTTPException(status_code=409, detail=str(e))
    return RecordingStatus(
        recording=True,
        filename=recorder.current_file,
        started_at=recorder.started_at.isoformat() if recorder.started_at else None,
    )


@app.post("/api/stop", response_model=RecordingStatus)
def stop_recording(background_tasks: BackgroundTasks):
    try:
        filepath = Path(recorder.stop())
    except RuntimeError as e:
        raise HTTPException(status_code=409, detail=str(e))
    _transcribing[filepath.name] = "pending"
    _transcribe_errors.pop(filepath.name, None)
    background_tasks.add_task(_do_transcribe, filepath.name)
    return RecordingStatus(recording=False, filename=filepath.name)


@app.get("/api/recordings", response_model=list[RecordingInfo])
def list_recordings():
    RECORDINGS_DIR.mkdir(exist_ok=True)
    filenames = {f.name for ext in ("*.mp3", "*.wav") for f in RECORDINGS_DIR.glob(ext)}
    filenames.update(row["filename"] for row in db.list_transcriptions())
    recordings = [_recording_info(filename) for filename in filenames]
    return sorted(recordings, key=lambda r: r.created_at, reverse=True)


@app.delete("/api/recordings/{filename}")
def delete_recording(filename: str):
    _require_idle(filename)
    _recording_info(filename)
    _recording_path(filename).unlink(missing_ok=True)
    db.delete_transcription(filename)
    db.delete_metadata(filename)
    _transcribing.pop(filename, None)
    _transcribe_errors.pop(filename, None)
    return {"detail": "deleted"}


@app.patch("/api/recordings/{filename}", response_model=RecordingInfo)
def rename_recording(filename: str, body: RenameRequest):
    _require_idle(filename)
    _recording_info(filename)
    title = body.new_name.strip()
    if not title or len(title) > 160:
        raise HTTPException(status_code=400, detail="Title must contain 1 to 160 characters")
    db.set_title(filename, title)
    return _recording_info(filename)


@app.get("/api/recordings/{filename}/download")
def download_recording(filename: str):
    filepath = _recording_path(filename)
    if not filepath.exists() or not filepath.is_file():
        raise HTTPException(status_code=404, detail="Recording not found")
    media = "audio/mpeg" if filepath.suffix == ".mp3" else "audio/wav"
    return FileResponse(filepath, media_type=media, filename=filename)


# ----- Transcription Endpoints -----

def _do_transcribe(filename: str):
    """Save the transcript before any destructive audio cleanup."""
    filepath = _recording_path(filename)
    settings = db.get_settings()
    try:
        _recording_info(filename)
        db.set_processing_error(filename, None)
        result = transcribe_file(filepath)
        db.save_transcription(filename, result["text"], result.get("language"))
    except Exception as exc:
        _transcribing[filename] = "error"
        _transcribe_errors[filename] = str(exc)
        return

    errors = []
    if settings["auto_delete_audio"]:
        try:
            filepath.unlink(missing_ok=True)
        except OSError:
            errors.append("Transcript saved, but audio could not be deleted. You can remove it manually.")
    if settings["auto_title"]:
        try:
            db.set_title(filename, suggest_title(result["text"]))
        except Exception as exc:
            errors.append(str(exc))
    db.set_processing_error(filename, " ".join(errors) or None)
    _transcribing[filename] = "completed"


@app.post("/api/recordings/{filename}/transcribe", response_model=TranscriptionStatus)
def start_transcription(filename: str, background_tasks: BackgroundTasks):
    filepath = _recording_path(filename)

    # If already transcribed, return it
    existing = db.get_transcription(filename)
    if existing:
        return TranscriptionStatus(
            filename=filename,
            status="completed",
            text=existing["text"],
            language=existing.get("language"),
        )

    # If already in progress, report pending
    if _transcribing.get(filename) == "pending":
        return TranscriptionStatus(filename=filename, status="pending")

    if not filepath.is_file():
        raise HTTPException(status_code=404, detail="Audio not found")

    # Launch background transcription
    _transcribing[filename] = "pending"
    _transcribe_errors.pop(filename, None)
    background_tasks.add_task(_do_transcribe, filename)
    return TranscriptionStatus(filename=filename, status="pending")


@app.get("/api/recordings/{filename}/transcription", response_model=TranscriptionStatus)
def get_transcription(filename: str):
    # Check DB first
    existing = db.get_transcription(filename)
    if existing:
        return TranscriptionStatus(
            filename=filename,
            title=db.get_metadata(filename).get("title"),
            status="completed",
            text=existing["text"],
            language=existing.get("language"),
        )

    # Check in-flight status
    status = _transcribing.get(filename)
    if status == "pending":
        return TranscriptionStatus(filename=filename, status="pending")
    if status == "error":
        return TranscriptionStatus(
            filename=filename,
            status="error",
            error=_transcribe_errors.get(filename, "Unknown error"),
        )

    return TranscriptionStatus(filename=filename, status="none")


@app.put("/api/recordings/{filename}/transcription", response_model=TranscriptionStatus)
def update_transcription(filename: str, body: UpdateTranscriptionRequest):
    """Update transcription text (e.g. after speaker rename)."""
    existing = db.get_transcription(filename)
    if not existing:
        raise HTTPException(status_code=404, detail="Transcription not found")
    db.update_transcription_text(filename, body.text)
    updated = db.get_transcription(filename)
    return TranscriptionStatus(
        filename=filename,
        status="completed",
        text=updated["text"],
        language=updated.get("language"),
    )


@app.get("/api/transcriptions/search", response_model=list[SearchResult])
def search_transcriptions(q: str):
    if not q.strip():
        return []
    results = db.search_transcriptions(q)
    return [
        SearchResult(
            filename=r["filename"],
            title=db.get_metadata(r["filename"]).get("title"),
            snippet=r["snippet"],
            language=r.get("language"),
        )
        for r in results
    ]


# ----- Static Frontend Files -----

FRONTEND_DIST = Path(__file__).parent.parent.parent / "grabby-front" / "dist"

# Mount static assets (JS, CSS, etc.)
if FRONTEND_DIST.exists():
    app.mount("/assets", StaticFiles(directory=FRONTEND_DIST / "assets"), name="assets")
    
    # Catch-all for SPA routing: serve index.html for all non-API routes
    @app.get("/{full_path:path}")
    def serve_frontend(full_path: str):
        # Don't serve index.html for API routes (already handled above)
        if full_path.startswith("api/"):
            raise HTTPException(status_code=404, detail="Not found")
        
        # Try to serve the requested file
        file_path = FRONTEND_DIST / full_path
        if file_path.is_file():
            return FileResponse(file_path)
        
        # Fall back to index.html for client-side routing
        index_path = FRONTEND_DIST / "index.html"
        if index_path.exists():
            return FileResponse(index_path)
        
        raise HTTPException(status_code=404, detail="Frontend not built")
