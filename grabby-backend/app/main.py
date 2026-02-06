from datetime import datetime
from pathlib import Path

from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse
from pydantic import BaseModel

from .recorder import recorder, RECORDINGS_DIR

app = FastAPI(title="Grabby – Meeting Recorder")

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
    new_filename = new_name if new_name.endswith(ext) else f"{new_name}{ext}"
    new_path = RECORDINGS_DIR / new_filename

    if new_path.exists():
        raise HTTPException(status_code=409, detail="A recording with that name already exists")

    filepath.rename(new_path)
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
