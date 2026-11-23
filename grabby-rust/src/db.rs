use std::cell::RefCell;
use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::{config, timefmt};

pub type DbResult<T> = rusqlite::Result<T>;

thread_local! {
    static CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

fn open() -> DbResult<Connection> {
    let conn = Connection::open(config::db_path())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    Ok(conn)
}

pub fn with_conn<T>(f: impl FnOnce(&Connection) -> DbResult<T>) -> DbResult<T> {
    CONN.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(open()?);
        }
        f(slot.as_ref().expect("connection just opened"))
    })
}

pub fn close_connection() {
    CONN.with(|cell| {
        cell.borrow_mut().take();
    });
}

pub fn init_db() -> DbResult<()> {
    with_conn(|conn| {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS processing_jobs (filename TEXT PRIMARY KEY, state TEXT NOT NULL);
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
        "#,
        )?;
        crate::sync::initialize(conn)
    })
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Transcription {
    pub id: i64,
    pub filename: String,
    pub text: String,
    pub language: Option<String>,
    pub created_at: String,
}

pub fn save_transcription(filename: &str, text: &str, language: Option<&str>) -> DbResult<i64> {
    with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM transcriptions WHERE filename = ?", [filename])?;
        tx.execute(
            "INSERT INTO transcriptions (filename, text, language, created_at) VALUES (?, ?, ?, ?)",
            params![filename, text, language, timefmt::now_utc()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    })
}

pub fn get_transcription_with(conn: &Connection, filename: &str) -> DbResult<Option<Transcription>> {
    conn.query_row(
        "SELECT id, filename, text, language, created_at FROM transcriptions WHERE filename = ?",
        [filename],
        |r| Ok(Transcription { id: r.get(0)?, filename: r.get(1)?, text: r.get(2)?, language: r.get(3)?, created_at: r.get(4)? }),
    )
    .optional()
}

pub fn get_transcription(filename: &str) -> DbResult<Option<Transcription>> {
    with_conn(|conn| get_transcription_with(conn, filename))
}

#[derive(Clone, Debug)]
pub struct SearchRow {
    pub filename: String,
    pub language: Option<String>,
    pub snippet: String,
}

pub fn search_transcriptions(query: &str) -> DbResult<Vec<SearchRow>> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let match_query = terms.iter().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect::<Vec<_>>().join(" AND ");
    with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t.filename, t.language,
                    snippet(transcriptions_fts, 1, '**', '**', '…', 40) AS snippet
             FROM transcriptions_fts fts
             JOIN transcriptions t ON t.id = fts.rowid
             WHERE transcriptions_fts MATCH ?
             ORDER BY rank",
        )?;
        let rows = stmt.query_map([match_query], |r| Ok(SearchRow { filename: r.get(0)?, language: r.get(1)?, snippet: r.get(2)? }))?;
        rows.collect()
    })
}

pub fn delete_transcription(filename: &str) -> DbResult<()> {
    with_conn(|conn| conn.execute("DELETE FROM transcriptions WHERE filename = ?", [filename]).map(|_| ()))
}

pub fn update_transcription_text(filename: &str, text: &str) -> DbResult<bool> {
    with_conn(|conn| Ok(conn.execute("UPDATE transcriptions SET text = ? WHERE filename = ?", [text, filename])? > 0))
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
pub struct RecordingSettings {
    pub auto_delete_audio: bool,
    pub auto_title: bool,
}

pub fn get_settings() -> DbResult<RecordingSettings> {
    with_conn(|conn| {
        let mut values = RecordingSettings::default();
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (key, value) = row?;
            match key.as_str() {
                "auto_delete_audio" => values.auto_delete_audio = value != 0,
                "auto_title" => values.auto_title = value != 0,
                _ => {}
            }
        }
        Ok(values)
    })
}

pub fn get_appearance() -> DbResult<i64> {
    with_conn(|conn| {
        Ok(conn
            .query_row("SELECT value FROM settings WHERE key = 'appearance'", [], |r| r.get::<_, i64>(0))
            .optional()?
            .filter(|v| (0..=2).contains(v))
            .unwrap_or(0))
    })
}

pub fn set_appearance(value: i64) -> DbResult<()> {
    with_conn(|conn| conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('appearance', ?)", [value]).map(|_| ()))
}

pub fn save_settings(values: RecordingSettings) -> DbResult<RecordingSettings> {
    with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        for (key, value) in [("auto_delete_audio", values.auto_delete_audio), ("auto_title", values.auto_title)] {
            tx.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)", params![key, value as i64])?;
        }
        tx.commit()
    })?;
    get_settings()
}

pub fn list_transcription_filenames() -> DbResult<Vec<String>> {
    with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT filename FROM transcriptions")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    })
}

pub fn list_metadata_filenames() -> DbResult<Vec<String>> {
    with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT filename FROM recording_metadata")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    })
}

pub fn ensure_metadata(filename: &str, created_at: &str) -> DbResult<()> {
    with_conn(|conn| {
        conn.execute("INSERT OR IGNORE INTO recording_metadata (filename, created_at) VALUES (?, ?)", [filename, created_at])
            .map(|_| ())
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub filename: String,
    pub title: Option<String>,
    pub created_at: String,
    pub processing_error: Option<String>,
}

pub fn get_metadata_with(conn: &Connection, filename: &str) -> DbResult<Option<Metadata>> {
    conn.query_row(
        "SELECT filename, title, created_at, processing_error FROM recording_metadata WHERE filename = ?",
        [filename],
        |r| Ok(Metadata { filename: r.get(0)?, title: r.get(1)?, created_at: r.get(2)?, processing_error: r.get(3)? }),
    )
    .optional()
}

pub fn get_metadata(filename: &str) -> DbResult<Option<Metadata>> {
    with_conn(|conn| get_metadata_with(conn, filename))
}

pub fn set_title(filename: &str, title: &str) -> DbResult<()> {
    with_conn(|conn| conn.execute("UPDATE recording_metadata SET title = ? WHERE filename = ?", [title, filename]).map(|_| ()))
}

pub fn set_processing_error(filename: &str, error: Option<&str>) -> DbResult<()> {
    with_conn(|conn| {
        conn.execute("UPDATE recording_metadata SET processing_error = ? WHERE filename = ?", params![error, filename]).map(|_| ())
    })
}

pub fn delete_metadata(filename: &str) -> DbResult<()> {
    with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM recording_metadata WHERE filename = ?", [filename])?;
        tx.execute("DELETE FROM recording_notes WHERE filename = ?", [filename])?;
        tx.execute("DELETE FROM processing_jobs WHERE filename = ?", [filename])?;
        tx.commit()
    })
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Notes {
    pub id: String,
    pub text: String,
}

pub fn create_notes(filename: Option<&str>) -> DbResult<Notes> {
    with_conn(|conn| {
        let notes_id = uuid::Uuid::new_v4().to_string();
        conn.execute("INSERT OR IGNORE INTO recording_notes (id, filename) VALUES (?, ?)", params![notes_id, filename])?;
        conn.query_row(
            "SELECT id, text FROM recording_notes WHERE filename = ? OR id = ?",
            params![filename, notes_id],
            |r| Ok(Notes { id: r.get(0)?, text: r.get(1)? }),
        )
    })
}

pub fn attach_notes(notes_id: &str, filename: &str) -> DbResult<()> {
    with_conn(|conn| conn.execute("UPDATE recording_notes SET filename = ? WHERE id = ?", [filename, notes_id]).map(|_| ()))
}

pub fn delete_notes(notes_id: &str) -> DbResult<()> {
    with_conn(|conn| conn.execute("DELETE FROM recording_notes WHERE id = ?", [notes_id]).map(|_| ()))
}

pub fn get_notes(notes_id: &str) -> DbResult<Option<Notes>> {
    with_conn(|conn| {
        conn.query_row("SELECT id, text FROM recording_notes WHERE id = ?", [notes_id], |r| Ok(Notes { id: r.get(0)?, text: r.get(1)? }))
            .optional()
    })
}

pub fn save_notes(notes_id: &str, text: &str) -> DbResult<bool> {
    with_conn(|conn| Ok(conn.execute("UPDATE recording_notes SET text = ? WHERE id = ?", [text, notes_id])? > 0))
}

pub fn set_job_state(filename: &str, state: &str) -> DbResult<()> {
    with_conn(|conn| conn.execute("INSERT OR REPLACE INTO processing_jobs VALUES (?, ?)", [filename, state]).map(|_| ()))
}

pub fn recover_jobs() -> DbResult<()> {
    with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE recording_metadata SET processing_error = 'Processing interrupted by backend restart. Retry transcription if no transcript is available.' WHERE filename IN (SELECT filename FROM processing_jobs WHERE state = 'pending')",
            [],
        )?;
        tx.execute("UPDATE processing_jobs SET state = 'interrupted' WHERE state = 'pending'", [])?;
        tx.commit()
    })
}

pub fn deleted_filenames() -> DbResult<Vec<String>> {
    with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT filename FROM sync_records WHERE deleted_at IS NOT NULL")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    })
}

pub fn is_deleted(filename: &str) -> DbResult<bool> {
    with_conn(|conn| {
        Ok(conn
            .query_row("SELECT 1 FROM sync_records WHERE filename=? AND deleted_at IS NOT NULL", [filename], |_| Ok(()))
            .optional()?
            .is_some())
    })
}

pub fn titles() -> DbResult<HashMap<String, Option<String>>> {
    with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT filename, title FROM recording_metadata")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })
}
