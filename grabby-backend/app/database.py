"""SQLite database with FTS5 for storing transcriptions."""

import sqlite3
import threading
import uuid
from pathlib import Path
from datetime import datetime

DB_PATH = Path(__file__).resolve().parent.parent / "grabby.db"

_local = threading.local()


def _get_conn() -> sqlite3.Connection:
    """Return a thread-local connection (SQLite objects are per-thread)."""
    if not hasattr(_local, "conn") or _local.conn is None:
        conn = sqlite3.connect(str(DB_PATH), check_same_thread=False)
        conn.row_factory = sqlite3.Row
        try:
            conn.execute("PRAGMA journal_mode=WAL")
        except sqlite3.Error:
            conn.close()
            raise
        _local.conn = conn
    return _local.conn


def close_connection() -> None:
    """Close this thread's connection, including after failed initialization."""
    conn = getattr(_local, "conn", None)
    _local.conn = None
    if conn is not None:
        conn.close()


def init_db() -> None:
    """Create tables and FTS virtual table if they don't exist."""
    conn = _get_conn()
    conn.executescript("""
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS recording_notes (
            id TEXT PRIMARY KEY,
            filename TEXT UNIQUE,
            text TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS recording_metadata (
            filename TEXT PRIMARY KEY,
            title TEXT,
            created_at TEXT NOT NULL,
            processing_error TEXT
        );
        CREATE TABLE IF NOT EXISTS transcriptions (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            filename    TEXT NOT NULL UNIQUE,
            text        TEXT NOT NULL,
            language    TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS transcriptions_fts USING fts5(
            filename,
            text,
            content='transcriptions',
            content_rowid='id'
        );

        -- Triggers to keep FTS index in sync
        CREATE TRIGGER IF NOT EXISTS transcriptions_ai AFTER INSERT ON transcriptions BEGIN
            INSERT INTO transcriptions_fts(rowid, filename, text)
            VALUES (new.id, new.filename, new.text);
        END;

        CREATE TRIGGER IF NOT EXISTS transcriptions_ad AFTER DELETE ON transcriptions BEGIN
            INSERT INTO transcriptions_fts(transcriptions_fts, rowid, filename, text)
            VALUES ('delete', old.id, old.filename, old.text);
        END;

        CREATE TRIGGER IF NOT EXISTS transcriptions_au AFTER UPDATE ON transcriptions BEGIN
            INSERT INTO transcriptions_fts(transcriptions_fts, rowid, filename, text)
            VALUES ('delete', old.id, old.filename, old.text);
            INSERT INTO transcriptions_fts(rowid, filename, text)
            VALUES (new.id, new.filename, new.text);
        END;
    """)
    conn.commit()


def save_transcription(filename: str, text: str, language: str | None = None) -> int:
    """Insert or replace a transcription. Returns the row id."""
    conn = _get_conn()
    # Delete existing if re-transcribing
    conn.execute("DELETE FROM transcriptions WHERE filename = ?", (filename,))
    cur = conn.execute(
        "INSERT INTO transcriptions (filename, text, language, created_at) VALUES (?, ?, ?, ?)",
        (filename, text, language, datetime.utcnow().isoformat()),
    )
    conn.commit()
    return cur.lastrowid  # type: ignore[return-value]


def get_transcription(filename: str) -> dict | None:
    """Get transcription for a recording filename."""
    conn = _get_conn()
    row = conn.execute(
        "SELECT id, filename, text, language, created_at FROM transcriptions WHERE filename = ?",
        (filename,),
    ).fetchone()
    if row is None:
        return None
    return dict(row)


def search_transcriptions(query: str) -> list[dict]:
    """Search literal user terms, requiring each whitespace-separated term."""
    terms = query.split()
    if not terms:
        return []
    match_query = " AND ".join('"' + term.replace('"', '""') + '"' for term in terms)
    conn = _get_conn()
    rows = conn.execute(
        """
        SELECT t.id, t.filename, t.language, t.created_at,
               snippet(transcriptions_fts, 1, '**', '**', '…', 40) AS snippet
        FROM transcriptions_fts fts
        JOIN transcriptions t ON t.id = fts.rowid
        WHERE transcriptions_fts MATCH ?
        ORDER BY rank
        """,
        (match_query,),
    ).fetchall()
    return [dict(r) for r in rows]


def delete_transcription(filename: str) -> None:
    """Delete transcription for a given filename."""
    conn = _get_conn()
    conn.execute("DELETE FROM transcriptions WHERE filename = ?", (filename,))
    conn.commit()


def rename_transcription(old_filename: str, new_filename: str) -> None:
    """Update filename in transcription when a recording is renamed."""
    conn = _get_conn()
    conn.execute(
        "UPDATE transcriptions SET filename = ? WHERE filename = ?",
        (new_filename, old_filename),
    )
    conn.commit()


def update_transcription_text(filename: str, text: str) -> bool:
    """Update the transcription text for a given filename. Returns True if found."""
    conn = _get_conn()
    cur = conn.execute(
        "UPDATE transcriptions SET text = ? WHERE filename = ?",
        (text, filename),
    )
    conn.commit()
    return cur.rowcount > 0


def get_settings() -> dict:
    values = {"auto_delete_audio": False, "auto_title": False}
    for row in _get_conn().execute("SELECT key, value FROM settings"):
        if row["key"] in values:
            values[row["key"]] = bool(row["value"])
    return values


def save_settings(values: dict) -> dict:
    conn = _get_conn()
    with conn:
        conn.executemany("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)",
                         [(key, int(value)) for key, value in values.items()])
    return get_settings()


def list_transcriptions() -> list[dict]:
    return [dict(row) for row in _get_conn().execute(
        "SELECT filename, created_at FROM transcriptions")]


def ensure_metadata(filename: str, created_at: str) -> None:
    conn = _get_conn()
    with conn:
        conn.execute("INSERT OR IGNORE INTO recording_metadata (filename, created_at) VALUES (?, ?)",
                     (filename, created_at))


def get_metadata(filename: str) -> dict:
    row = _get_conn().execute("SELECT * FROM recording_metadata WHERE filename = ?", (filename,)).fetchone()
    return dict(row) if row else {}


def set_title(filename: str, title: str) -> None:
    conn = _get_conn()
    with conn:
        conn.execute("UPDATE recording_metadata SET title = ? WHERE filename = ?", (title, filename))


def set_processing_error(filename: str, error: str | None) -> None:
    conn = _get_conn()
    with conn:
        conn.execute("UPDATE recording_metadata SET processing_error = ? WHERE filename = ?", (error, filename))


def delete_metadata(filename: str) -> None:
    conn = _get_conn()
    with conn:
        conn.execute("DELETE FROM recording_metadata WHERE filename = ?", (filename,))
        conn.execute("DELETE FROM recording_notes WHERE filename = ?", (filename,))


def create_notes(filename: str | None = None) -> dict:
    conn = _get_conn()
    notes_id = str(uuid.uuid4())
    with conn:
        conn.execute("INSERT OR IGNORE INTO recording_notes (id, filename) VALUES (?, ?)",
                     (notes_id, filename))
    row = conn.execute("SELECT id, text FROM recording_notes WHERE filename = ? OR id = ?",
                       (filename, notes_id)).fetchone()
    return dict(row)


def attach_notes(notes_id: str, filename: str) -> None:
    with _get_conn() as conn:
        conn.execute("UPDATE recording_notes SET filename = ? WHERE id = ?", (filename, notes_id))


def get_notes(notes_id: str) -> dict | None:
    row = _get_conn().execute("SELECT id, text FROM recording_notes WHERE id = ?", (notes_id,)).fetchone()
    return dict(row) if row else None


def save_notes(notes_id: str, text: str) -> bool:
    with _get_conn() as conn:
        result = conn.execute("UPDATE recording_notes SET text = ? WHERE id = ?", (text, notes_id))
    return result.rowcount > 0
