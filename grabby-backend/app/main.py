from datetime import datetime
from pathlib import Path

from dotenv import load_dotenv
from fastapi import BackgroundTasks, FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse
from pydantic import BaseModel

from .recorder import recorder, RECORDINGS_DIR
from . import database as db
from .transcription import transcribe_file

load_dotenv()  # reads .env from CWD (grabby-backend/)

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

class StartRequest(BaseModel):
    name: str | None = None


class RecordingStatus(BaseModel):
    recording: bool
    filename: str | None = None
    started_at: str | None = None


class RecordingInfo(BaseModel):
    filename: str
    name: str
    size_bytes: int
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
    status: str  # "pending" | "completed" | "error"
    text: str | None = None
    language: str | None = None
    error: str | None = None


class SearchResult(BaseModel):
    filename: str
    snippet: str
    language: str | None = None


class UpdateTranscriptionRequest(BaseModel):
    text: str


# Track in-flight transcription jobs
_transcribing: dict[str, str] = {}  # filename -> status ("pending"|"error")
_transcribe_errors: dict[str, str] = {}  # filename -> error message


# ----- Endpoints -----

@app.get("/api/status", response_model=RecordingStatus)
def get_status():
    return RecordingStatus(
        recording=recorder.is_recording,
        filename=recorder.current_file,
        started_at=recorder.started_at.isoformat() if recorder.started_at else None,
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
def stop_recording():
    try:
        recorder.stop()
    except RuntimeError as e:
        raise HTTPException(status_code=409, detail=str(e))
    return RecordingStatus(recording=False)


@app.get("/api/recordings", response_model=list[RecordingInfo])
def list_recordings():
    RECORDINGS_DIR.mkdir(exist_ok=True)
    recordings: list[RecordingInfo] = []
    for ext in ("*.mp3", "*.wav"):
        for f in RECORDINGS_DIR.glob(ext):
            stat = f.stat()
            recordings.append(
                RecordingInfo(
                    filename=f.name,
                    name=f.stem,
                    size_bytes=stat.st_size,
                    created_at=datetime.fromtimestamp(stat.st_mtime).isoformat(),
                )
            )
    recordings.sort(key=lambda r: r.created_at, reverse=True)
    return recordings


@app.delete("/api/recordings/{filename}")
def delete_recording(filename: str):
    filepath = RECORDINGS_DIR / filename
    if not filepath.exists() or not filepath.is_file():
        raise HTTPException(status_code=404, detail="Recording not found")
    filepath.unlink()
    db.delete_transcription(filename)
    return {"detail": "deleted"}


@app.patch("/api/recordings/{filename}", response_model=RecordingInfo)
def rename_recording(filename: str, body: RenameRequest):
    filepath = RECORDINGS_DIR / filename
    if not filepath.exists() or not filepath.is_file():
        raise HTTPException(status_code=404, detail="Recording not found")

    new_name = body.new_name.strip()
    if not new_name:
        raise HTTPException(status_code=400, detail="Name cannot be empty")

    ext = filepath.suffix  # preserve original .mp3 or .wav
    stem = new_name.removesuffix(ext) if new_name.endswith(ext) else new_name

    # Avoid filename collision: append _1, _2, … if needed
    candidate = stem
    counter = 1
    while True:
        new_path = RECORDINGS_DIR / f"{candidate}{ext}"
        if not new_path.exists() or new_path == filepath:
            break
        candidate = f"{stem}_{counter}"
        counter += 1

    filepath.rename(new_path)
    db.rename_transcription(filename, new_path.name)
    stat = new_path.stat()
    return RecordingInfo(
        filename=new_path.name,
        name=new_path.stem,
        size_bytes=stat.st_size,
        created_at=datetime.fromtimestamp(stat.st_mtime).isoformat(),
    )


@app.get("/api/recordings/{filename}/download")
def download_recording(filename: str):
    filepath = RECORDINGS_DIR / filename
    if not filepath.exists() or not filepath.is_file():
        raise HTTPException(status_code=404, detail="Recording not found")
    media = "audio/mpeg" if filepath.suffix == ".mp3" else "audio/wav"
    return FileResponse(filepath, media_type=media, filename=filename)


# ----- Transcription Endpoints -----

def _do_transcribe(filename: str):
    """Background task: transcribe file and save to DB."""
    filepath = RECORDINGS_DIR / filename
    try:
        result = transcribe_file(filepath)
        db.save_transcription(filename, result["text"], result.get("language"))
        _transcribing[filename] = "completed"
    except Exception as exc:
        _transcribing[filename] = "error"
        _transcribe_errors[filename] = str(exc)


@app.post("/api/recordings/{filename}/transcribe", response_model=TranscriptionStatus)
def start_transcription(filename: str, background_tasks: BackgroundTasks):
    filepath = RECORDINGS_DIR / filename
    if not filepath.exists() or not filepath.is_file():
        raise HTTPException(status_code=404, detail="Recording not found")

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
            snippet=r["snippet"],
            language=r.get("language"),
        )
        for r in results
    ]
