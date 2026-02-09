"""SQLite database with FTS5 for storing transcriptions."""

import sqlite3
import threading
from pathlib import Path
from datetime import datetime

DB_PATH = Path(__file__).resolve().parent.parent / "grabby.db"

_local = threading.local()


def _get_conn() -> sqlite3.Connection:
    """Return a thread-local connection (SQLite objects are per-thread)."""
    if not hasattr(_local, "conn") or _local.conn is None:
        conn = sqlite3.connect(str(DB_PATH), check_same_thread=False)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode=WAL")
        _local.conn = conn
    return _local.conn


def init_db() -> None:
    """Create tables and FTS virtual table if they don't exist."""
    conn = _get_conn()
    conn.executescript("""
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
    """Full-text search across transcriptions. Returns matching rows with snippets."""
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
        (query,),
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
