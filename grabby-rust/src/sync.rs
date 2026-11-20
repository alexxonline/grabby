//! Text-only synchronization and persistent reference receiver (port of `app/sync.py`).
//! The wire contract is documented in `SYNC.md`.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{config, db, timefmt};

static LOCK: Mutex<()> = Mutex::new(());

/// A sync failure, identified by an error kind name: the status message
/// shows only that name, never provider responses, URLs or credentials.
#[derive(Debug, Clone)]
pub struct SyncError {
    pub kind: &'static str,
    pub message: String,
}

impl SyncError {
    pub fn value(message: impl Into<String>) -> Self {
        SyncError { kind: "ValueError", message: message.into() }
    }
    fn key(key: &str) -> Self {
        SyncError { kind: "KeyError", message: key.to_owned() }
    }
    fn type_(message: &str) -> Self {
        SyncError { kind: "TypeError", message: message.to_owned() }
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<rusqlite::Error> for SyncError {
    fn from(e: rusqlite::Error) -> Self {
        SyncError { kind: "OperationalError", message: e.to_string() }
    }
}

type SResult<T> = Result<T, SyncError>;

fn sync_lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn running() -> bool {
    matches!(LOCK.try_lock(), Err(std::sync::TryLockError::WouldBlock))
}

pub fn initialize(conn: &Connection) -> db::DbResult<()> {
    conn.execute_batch(
        "
    CREATE TABLE IF NOT EXISTS sync_config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sync_control (id INTEGER PRIMARY KEY, applying INTEGER NOT NULL);
    INSERT OR IGNORE INTO sync_control VALUES (1,0);
    CREATE TABLE IF NOT EXISTS sync_records (
      recording_id TEXT PRIMARY KEY, filename TEXT UNIQUE NOT NULL,
      updated_at TEXT NOT NULL, deleted_at TEXT, generation INTEGER NOT NULL DEFAULT 1,
      synced_generation INTEGER NOT NULL DEFAULT 0, revision INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS sync_conflicts (recording_id TEXT PRIMARY KEY, snapshot TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sync_mock_records (recording_id TEXT PRIMARY KEY, snapshot TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sync_mock_mutations (id TEXT PRIMARY KEY, acknowledgment TEXT NOT NULL);
    ",
    )?;
    for table in ["recording_metadata", "transcriptions", "recording_notes"] {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            let reference = if action == "DELETE" { "old" } else { "new" };
            let deletion = table == "recording_metadata" && action == "DELETE";
            let now = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";
            let deleted_value = if deletion { now } else { "NULL" };
            let deleted_update = if deletion || (table == "recording_metadata" && action == "INSERT") {
                "excluded.deleted_at"
            } else {
                "sync_records.deleted_at"
            };
            conn.execute_batch(&format!(
                "
            CREATE TRIGGER IF NOT EXISTS sync_{table}_{action} AFTER {action} ON {table}
            WHEN {reference}.filename IS NOT NULL AND (SELECT applying FROM sync_control WHERE id=1)=0
            BEGIN
              INSERT INTO sync_records(recording_id,filename,updated_at,deleted_at)
              VALUES(lower(hex(randomblob(16))),{reference}.filename,{now},
                {deleted_value})
              ON CONFLICT(filename) DO UPDATE SET generation=generation+1,
                updated_at=excluded.updated_at, deleted_at={deleted_update};
            END;
            "
            ))?;
        }
    }
    conn.execute(
        "INSERT OR IGNORE INTO sync_records(recording_id,filename,updated_at)
       SELECT lower(hex(randomblob(16))),filename,? FROM (
         SELECT filename FROM recording_metadata UNION SELECT filename FROM transcriptions
         UNION SELECT filename FROM recording_notes WHERE filename IS NOT NULL)",
        [timefmt::now_utc()],
    )?;
    conn.execute("INSERT OR IGNORE INTO sync_config VALUES ('device_id',?)", [uuid::Uuid::new_v4().to_string()])?;
    Ok(())
}

fn config_map(conn: &Connection) -> SResult<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT key,value FROM sync_config")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn put(conn: &Connection, key: &str, value: impl ToString) -> SResult<()> {
    conn.execute("INSERT OR REPLACE INTO sync_config VALUES (?,?)", [key, &value.to_string()])?;
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct SyncSettings {
    pub enabled: bool,
    pub endpoint: String,
    pub mock: bool,
    pub api_key_configured: bool,
    pub device_id: String,
    pub last_success: Option<String>,
    pub error: Option<String>,
    pub running: bool,
    pub conflicts: i64,
}

fn api_key() -> String {
    config::env_var("GRABBY_SYNC_API_KEY").unwrap_or_default()
}

pub fn settings() -> SResult<SyncSettings> {
    db::with_conn(|conn| Ok(settings_with(conn)))?
}

fn settings_with(conn: &Connection) -> SResult<SyncSettings> {
    let c = config_map(conn)?;
    let non_empty = |k: &str| c.get(k).filter(|v| !v.is_empty()).cloned();
    Ok(SyncSettings {
        enabled: c.get("enabled").map(String::as_str) == Some("1"),
        endpoint: c.get("endpoint").cloned().unwrap_or_default(),
        mock: c.get("mock").map(String::as_str) == Some("1"),
        api_key_configured: !api_key().is_empty(),
        device_id: c.get("device_id").cloned().ok_or_else(|| SyncError::key("device_id"))?,
        last_success: c.get("last_success").cloned(),
        error: non_empty("error"),
        running: running(),
        conflicts: conn.query_row("SELECT count(*) FROM sync_conflicts", [], |r| r.get(0))?,
    })
}

fn valid_endpoint(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else { return false };
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none_or(str::is_empty)
        && url.query().is_none_or(str::is_empty)
}

pub fn save_settings(enabled: bool, endpoint: &str, mock: bool, api_key_input: Option<&str>) -> SResult<SyncSettings> {
    let endpoint = endpoint.trim();
    if !mock && !endpoint.is_empty() && !valid_endpoint(endpoint) {
        return Err(SyncError::value("Use an HTTPS endpoint without credentials, query or fragment."));
    }
    let key = match api_key_input {
        Some(k) => k.trim().to_owned(),
        None => api_key(),
    };
    if enabled && (key.is_empty() || (!mock && endpoint.is_empty())) {
        return Err(SyncError::value("An endpoint and API key are required to enable sync."));
    }
    if key.contains('\r') || key.contains('\n') {
        return Err(SyncError::value("Invalid API key."));
    }
    {
        let _guard = sync_lock();
        if api_key_input.is_some() {
            // Encrypted with DPAPI into configs.dat; never written in plain text.
            crate::secrets::set(crate::secrets::SYNC, &key).map_err(|e| SyncError { kind: "OSError", message: e })?;
        }
        db::with_conn(|conn| {
            let r: SResult<()> = (|| {
                let c = config_map(conn)?;
                let tx = conn.unchecked_transaction()?;
                if c.get("endpoint").map(String::as_str).unwrap_or("") != endpoint || (c.get("mock").map(String::as_str) == Some("1")) != mock {
                    tx.execute("UPDATE sync_records SET revision=0,synced_generation=0,generation=generation+1", [])?;
                    tx.execute("DELETE FROM sync_conflicts", [])?;
                    put(&tx, "cursor", 0)?;
                    put(&tx, "last_success", "")?;
                }
                put(&tx, "enabled", enabled as i32)?;
                put(&tx, "endpoint", endpoint)?;
                put(&tx, "mock", mock as i32)?;
                put(&tx, "error", "")?;
                tx.commit()?;
                Ok(())
            })();
            Ok(r)
        })??;
    }
    settings()
}

#[derive(Clone, Debug)]
struct Row {
    recording_id: String,
    filename: String,
    updated_at: String,
    deleted_at: Option<String>,
    generation: i64,
    synced_generation: i64,
    revision: i64,
}

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        recording_id: r.get("recording_id")?,
        filename: r.get("filename")?,
        updated_at: r.get("updated_at")?,
        deleted_at: r.get("deleted_at")?,
        generation: r.get("generation")?,
        synced_generation: r.get("synced_generation")?,
        revision: r.get("revision")?,
    })
}

fn record_by_id(conn: &Connection, recording_id: &str) -> SResult<Option<Row>> {
    Ok(conn.query_row("SELECT * FROM sync_records WHERE recording_id=?", [recording_id], row_from).optional()?)
}

fn utc_str(value: &str, local: bool) -> SResult<String> {
    timefmt::to_utc(value, local).ok_or_else(|| SyncError::value(format!("Invalid isoformat string: {value:?}")))
}

fn snapshot(conn: &Connection, row: &Row) -> SResult<Value> {
    let filename = &row.filename;
    let metadata = db::get_metadata_with(conn, filename)?;
    let transcript = db::get_transcription_with(conn, filename)?;
    let notes: Option<(String, String)> =
        conn.query_row("SELECT id,text FROM recording_notes WHERE filename=?", [filename], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let created = metadata.as_ref().map(|m| m.created_at.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| row.updated_at.clone());
    Ok(json!({
        "recording_id": row.recording_id,
        "filename": filename,
        "created_at": utc_str(&created, metadata.is_some())?,
        "updated_at": utc_str(&row.updated_at, false)?,
        "deleted_at": row.deleted_at,
        "base_revision": row.revision,
        "mutation_id": format!("{}:{}", row.recording_id, row.generation),
        "title": metadata.and_then(|m| m.title),
        "transcription": match transcript {
            Some(t) => json!({"text": t.text, "language": t.language, "created_at": utc_str(&t.created_at, false)?}),
            None => Value::Null,
        },
        "notes": match notes { Some((id, text)) => json!({"id": id, "text": text}), None => Value::Null },
    }))
}

// ----- Helpers for access to JSON payloads -----

fn get<'a>(v: &'a Value, key: &str) -> SResult<&'a Value> {
    match v {
        Value::Object(m) => m.get(key).ok_or_else(|| SyncError::key(key)),
        Value::Array(_) => Err(SyncError::type_("list indices must be integers or slices, not str")),
        _ => Err(SyncError::type_("object is not subscriptable")),
    }
}

fn get_str<'a>(v: &'a Value, key: &str) -> SResult<&'a str> {
    get(v, key)?.as_str().ok_or_else(|| SyncError::type_("expected a string"))
}

fn get_int(v: &Value, key: &str) -> SResult<Option<i64>> {
    let x = get(v, key)?;
    Ok(if x.is_i64() || x.is_u64() { x.as_i64() } else { None })
}

fn get_array<'a>(v: &'a Value, key: &str) -> SResult<&'a Vec<Value>> {
    get(v, key)?.as_array().ok_or_else(|| SyncError::type_("expected a list"))
}

fn check_uuid(v: &Value) -> SResult<()> {
    let s = v.as_str().ok_or_else(|| SyncError::type_("one of the hex, bytes, bytes_le, fields, or int arguments must be given"))?;
    uuid::Uuid::parse_str(s).map(|_| ()).map_err(|_| SyncError::value("badly formed hexadecimal UUID string"))
}

fn check_utc(v: &Value) -> SResult<()> {
    let s = v.as_str().ok_or_else(|| SyncError::type_("expected a date string"))?;
    utc_str(s, false).map(|_| ())
}

fn validate_filename(filename: &str) -> bool {
    !filename.is_empty()
        && !filename.contains(['/', '\\', ':'])
        && Path::new(filename).file_name().and_then(|n| n.to_str()) == Some(filename)
}

fn validate_record(r: &Value, incoming: bool) -> SResult<()> {
    check_uuid(get(r, "recording_id")?)?;
    let filename = get(r, "filename")?;
    match filename.as_str() {
        Some(f) if validate_filename(f) => {}
        Some(_) => return Err(SyncError::value("Invalid filename")),
        None if filename.is_null() => return Err(SyncError::value("Invalid filename")),
        None => return Err(SyncError::type_("expected a string filename")),
    }
    for key in ["created_at", "updated_at"] {
        check_utc(get(r, key)?)?;
    }
    let deleted = get(r, "deleted_at")?;
    if !deleted.is_null() {
        check_utc(deleted)?;
    }
    let title = get(r, "title")?;
    if !title.is_null() && !title.is_string() {
        return Err(SyncError::value("Invalid title"));
    }
    for key in ["transcription", "notes"] {
        let part = get(r, key)?;
        if !part.is_null() && !get(part, "text")?.is_string() {
            return Err(SyncError::value("Invalid text"));
        }
    }
    let transcription = get(r, "transcription")?;
    if !transcription.is_null() {
        check_utc(get(transcription, "created_at")?)?;
    }
    let notes = get(r, "notes")?;
    if !notes.is_null() {
        check_uuid(get(notes, "id")?)?;
    }
    let rev = get_int(r, if incoming { "revision" } else { "base_revision" })?;
    if !rev.is_some_and(|v| v >= 0) {
        return Err(SyncError::value("Invalid revision"));
    }
    if !incoming && !get(r, "mutation_id")?.as_str().is_some_and(|s| !s.is_empty()) {
        return Err(SyncError::value("Invalid mutation"));
    }
    Ok(())
}

/// Mock receiver transaction: account is the single configured local test account.
pub fn receive(body: &Value) -> SResult<Value> {
    let schema = get(body, "schema_version")?;
    let cursor = get_int(body, "cursor")?;
    if schema.as_f64() != Some(1.0) && schema.as_bool() != Some(true) || !cursor.is_some_and(|c| c >= 0) {
        return Err(SyncError::value("Invalid sync request"));
    }
    let cursor = cursor.unwrap_or(0);
    check_uuid(get(body, "device_id")?)?;
    let device_id = get_str(body, "device_id")?;
    let incoming = get_array(body, "records")?;
    for r in incoming {
        validate_record(r, false)?;
    }
    db::with_conn(|conn| Ok(receive_with(conn, cursor, device_id, incoming)))?
}

fn receive_with(conn: &Connection, cursor: i64, device_id: &str, incoming: &[Value]) -> SResult<Value> {
    let tx = conn.unchecked_transaction()?;
    // Insertion-ordered.
    let mut records: Vec<(String, Value)> = {
        let mut stmt = tx.prepare("SELECT recording_id, snapshot FROM sync_mock_records")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, snap) = row?;
            out.push((id, serde_json::from_str(&snap).map_err(|e| SyncError::value(e.to_string()))?));
        }
        out
    };
    let rev_of = |v: &Value| v.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let mut revision = records.iter().map(|(_, r)| rev_of(r)).max().unwrap_or(0);
    if cursor > revision {
        return Err(SyncError::value("Cursor is ahead of this server"));
    }
    let mut acks = Vec::new();
    let mut conflicts = Vec::new();
    for r in incoming {
        let mutation_id = get_str(r, "mutation_id")?;
        let recording_id = get_str(r, "recording_id")?;
        let mutation = format!("{device_id}:{mutation_id}");
        let previous: Option<String> =
            tx.query_row("SELECT acknowledgment FROM sync_mock_mutations WHERE id=?", [&mutation], |row| row.get(0)).optional()?;
        if let Some(previous) = previous {
            acks.push(serde_json::from_str::<Value>(&previous).map_err(|e| SyncError::value(e.to_string()))?);
            continue;
        }
        let current = records.iter().find(|(id, _)| id == recording_id).map(|(_, v)| v.clone());
        let base = get_int(r, "base_revision")?.unwrap_or(-1);
        if base != current.as_ref().map(rev_of).unwrap_or(0) {
            match current {
                Some(c) => conflicts.push(c),
                None => return Err(SyncError::value("Unknown recording with nonzero base revision")),
            }
            continue;
        }
        revision += 1;
        let mut saved = Map::new();
        for k in ["recording_id", "filename", "created_at", "updated_at", "deleted_at", "title", "transcription", "notes"] {
            saved.insert(k.to_owned(), get(r, k)?.clone());
        }
        saved.insert("revision".to_owned(), json!(revision));
        let saved = Value::Object(saved);
        match records.iter_mut().find(|(id, _)| id == recording_id) {
            Some(slot) => slot.1 = saved.clone(),
            None => records.push((recording_id.to_owned(), saved.clone())),
        }
        let ack = json!({"recording_id": recording_id, "mutation_id": mutation_id, "revision": revision});
        tx.execute("INSERT OR REPLACE INTO sync_mock_records VALUES (?,?)", [recording_id, &saved.to_string()])?;
        tx.execute("INSERT INTO sync_mock_mutations VALUES (?,?)", [&mutation, &ack.to_string()])?;
        acks.push(ack);
    }
    tx.commit()?;
    let changes: Vec<Value> = records.into_iter().map(|(_, r)| r).filter(|r| rev_of(r) > cursor).collect();
    Ok(json!({"schema_version": 1, "cursor": revision, "acknowledgments": acks, "changes": changes, "conflicts": conflicts}))
}

fn exchange(body: &Value, c: &std::collections::HashMap<String, String>) -> SResult<Value> {
    if c.get("mock").map(String::as_str) == Some("1") {
        let failure = c.get("mock_failure").cloned().unwrap_or_default();
        if !failure.is_empty() {
            db::with_conn(|conn| Ok(put(conn, "mock_failure", "")))??;
            if failure == "authentication" {
                return Err(SyncError { kind: "HTTPError", message: "HTTP Error 401: Invalid API key".into() });
            }
            return Err(SyncError { kind: "TimeoutError", message: "Simulated mock failure".into() });
        }
        return receive(body);
    }
    let endpoint = c.get("endpoint").ok_or_else(|| SyncError::key("endpoint"))?;
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| SyncError { kind: "URLError", message: e.to_string() })?;
    let response = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key()))
        .body(body.to_string())
        .send()
        .map_err(|e| SyncError { kind: if e.is_timeout() { "TimeoutError" } else { "URLError" }, message: e.to_string() })?;
    let status = response.status();
    if !status.is_success() {
        return Err(SyncError { kind: "HTTPError", message: format!("HTTP Error {}", status.as_u16()) });
    }
    use std::io::Read;
    let mut raw = Vec::new();
    response
        .take(10_000_001)
        .read_to_end(&mut raw)
        .map_err(|e| SyncError { kind: "URLError", message: e.to_string() })?;
    if raw.len() > 10_000_000 {
        return Err(SyncError::value("Sync response exceeds limit"));
    }
    serde_json::from_slice(&raw).map_err(|e| SyncError { kind: "JSONDecodeError", message: e.to_string() })
}

fn apply_remote(conn: &Connection, r: &Value) -> SResult<()> {
    let recording_id = get_str(r, "recording_id")?;
    let existing = record_by_id(conn, recording_id)?;
    let mut filename = match &existing {
        Some(e) => e.filename.clone(),
        None => get_str(r, "filename")?.to_owned(),
    };
    let collision: Option<String> =
        conn.query_row("SELECT recording_id FROM sync_records WHERE filename=?", [&filename], |row| row.get(0)).optional()?;
    if existing.is_none() && (collision.is_some() || config::recordings_dir().join(&filename).exists()) {
        let path = Path::new(&filename);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_owned();
        let suffix = path.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
        filename = format!("{stem}-{recording_id}{suffix}");
    }
    conn.execute("UPDATE sync_control SET applying=1 WHERE id=1", [])?;
    let result: SResult<()> = (|| {
        conn.execute("DELETE FROM transcriptions WHERE filename=?", [&filename])?;
        conn.execute("DELETE FROM recording_notes WHERE filename=?", [&filename])?;
        conn.execute("DELETE FROM recording_metadata WHERE filename=?", [&filename])?;
        let deleted = get(r, "deleted_at")?;
        let deleted_truthy = !(deleted.is_null() || deleted.as_str() == Some(""));
        if !deleted_truthy {
            conn.execute(
                "INSERT INTO recording_metadata(filename,title,created_at) VALUES (?,?,?)",
                params![filename, get(r, "title")?.as_str(), get_str(r, "created_at")?],
            )?;
            let t = get(r, "transcription")?;
            if t.is_object() && !t.as_object().is_some_and(Map::is_empty) {
                conn.execute(
                    "INSERT INTO transcriptions(filename,text,language,created_at) VALUES (?,?,?,?)",
                    params![filename, get_str(t, "text")?, t.get("language").and_then(Value::as_str), get_str(t, "created_at")?],
                )?;
            }
            let n = get(r, "notes")?;
            if n.is_object() && !n.as_object().is_some_and(Map::is_empty) {
                // Notes IDs can collide across independently imported recordings; retain both texts.
                let mut nid = get_str(n, "id")?.to_owned();
                if conn.query_row("SELECT 1 FROM recording_notes WHERE id=?", [&nid], |_| Ok(())).optional()?.is_some() {
                    nid = uuid::Uuid::new_v4().to_string();
                }
                conn.execute("INSERT INTO recording_notes VALUES (?,?,?)", params![nid, filename, get_str(n, "text")?])?;
            }
        }
        conn.execute(
            "INSERT INTO sync_records VALUES (?,?,?,?,1,1,?)
          ON CONFLICT(recording_id) DO UPDATE SET updated_at=excluded.updated_at,
          deleted_at=excluded.deleted_at,revision=excluded.revision,synced_generation=generation",
            params![recording_id, filename, get_str(r, "updated_at")?, deleted.as_str(), get_int(r, "revision")?],
        )?;
        Ok(())
    })();
    conn.execute("UPDATE sync_control SET applying=0 WHERE id=1", [])?;
    result
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RunResult {
    pub uploaded: usize,
    pub downloaded: usize,
    #[serde(flatten)]
    pub settings: SyncSettings,
}

pub fn run() -> SResult<RunResult> {
    let guard = match LOCK.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return Err(SyncError::value("Sync is already running.")),
    };
    let result = run_locked();
    if let Err(e) = &result {
        // Never expose provider responses, URLs or credentials in logs/status.
        let message = format!("Sync failed ({}). Check connection and credentials.", e.kind);
        let _ = db::with_conn(|conn| Ok(put(conn, "error", message)));
    }
    let result = result.and_then(|(uploaded, downloaded)| {
        let mut settings = db::with_conn(|conn| Ok(settings_with(conn)))??;
        settings.running = false;
        Ok(RunResult { uploaded, downloaded, settings })
    });
    drop(guard);
    result
}

fn run_locked() -> SResult<(usize, usize)> {
    let c = db::with_conn(|conn| Ok(config_map(conn)))??;
    if c.get("enabled").map(String::as_str) != Some("1") {
        return Err(SyncError::value("Enable sync in Settings first."));
    }
    let pending: Vec<Row> = db::with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT * FROM sync_records WHERE generation>synced_generation
                    AND recording_id NOT IN (SELECT recording_id FROM sync_conflicts)",
        )?;
        let rows = stmt.query_map([], row_from)?;
        rows.collect::<Result<Vec<_>, _>>()
    })?;
    let sent: Vec<Value> = db::with_conn(|conn| Ok(pending.iter().map(|r| snapshot(conn, r)).collect::<SResult<Vec<_>>>()))??;
    let cursor: i64 = c.get("cursor").and_then(|v| v.parse().ok()).unwrap_or(0);
    let body = json!({"schema_version": 1, "device_id": c.get("device_id").cloned().unwrap_or_default(), "cursor": cursor, "records": sent});
    let response = exchange(&body, &c)?;
    let response_cursor = get_int(&response, "cursor")?;
    if get(&response, "schema_version")?.as_f64() != Some(1.0) || !response_cursor.is_some_and(|rc| rc >= cursor) {
        return Err(SyncError::value("Invalid response cursor"));
    }
    let response_cursor = response_cursor.unwrap_or(0);
    let changes = get_array(&response, "changes")?;
    let conflicts = get_array(&response, "conflicts")?;
    for r in changes.iter().chain(conflicts) {
        validate_record(r, true)?;
        let rev = get_int(r, "revision")?.unwrap_or(0);
        if rev <= 0 || rev > response_cursor {
            return Err(SyncError::value("Invalid snapshot revision"));
        }
    }
    let acks = get_array(&response, "acknowledgments")?;
    db::with_conn(|conn| {
        Ok((|| -> SResult<()> {
            let tx = conn.unchecked_transaction()?;
            for ack in acks {
                let mutation_id = get(ack, "mutation_id")?;
                let original = sent.iter().find(|s| s.get("mutation_id") == Some(mutation_id));
                let ack_rev = get_int(ack, "revision")?;
                let valid = match (original, ack_rev) {
                    (Some(o), Some(rev)) => {
                        Some(get(ack, "recording_id")?) == o.get("recording_id")
                            && rev > o.get("base_revision").and_then(Value::as_i64).unwrap_or(0)
                            && rev <= response_cursor
                    }
                    _ => false,
                };
                if !valid {
                    return Err(SyncError::value("Invalid acknowledgment"));
                }
                let recording_id = get_str(ack, "recording_id")?;
                let generation = pending
                    .iter()
                    .find(|r| r.recording_id == recording_id)
                    .map(|r| r.generation)
                    .ok_or_else(|| SyncError { kind: "StopIteration", message: String::new() })?;
                tx.execute(
                    "UPDATE sync_records SET synced_generation=?,revision=? WHERE recording_id=?",
                    params![generation, ack_rev, recording_id],
                )?;
            }
            for r in conflicts.iter().chain(changes) {
                let recording_id = get_str(r, "recording_id")?;
                let local = record_by_id(&tx, recording_id)?;
                let rev = get_int(r, "revision")?.unwrap_or(0);
                if let Some(l) = &local {
                    if rev <= l.revision {
                        continue;
                    }
                }
                if local.as_ref().is_some_and(|l| l.generation > l.synced_generation) {
                    tx.execute("INSERT OR REPLACE INTO sync_conflicts VALUES (?,?)", [recording_id, &r.to_string()])?;
                } else {
                    apply_remote(&tx, r)?;
                }
            }
            put(&tx, "cursor", response_cursor)?;
            put(&tx, "last_success", timefmt::now_utc())?;
            put(&tx, "error", "")?;
            tx.commit()?;
            Ok(())
        })())
    })??;
    Ok((acks.len(), changes.len()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub recording_id: String,
    pub local: Value,
    pub server: Value,
}

pub fn conflicts() -> SResult<Vec<Conflict>> {
    db::with_conn(|conn| {
        Ok((|| -> SResult<Vec<Conflict>> {
            let rows: Vec<(String, String)> = {
                let mut stmt = conn.prepare("SELECT recording_id, snapshot FROM sync_conflicts")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect::<Result<_, _>>()?
            };
            let mut out = Vec::new();
            for (recording_id, snap) in rows {
                let local = record_by_id(conn, &recording_id)?.ok_or_else(|| SyncError::type_("local record missing"))?;
                out.push(Conflict {
                    local: snapshot(conn, &local)?,
                    server: serde_json::from_str(&snap).map_err(|e| SyncError::value(e.to_string()))?,
                    recording_id,
                });
            }
            Ok(out)
        })())
    })?
}

pub fn resolve(recording_id: &str, choice: &str) -> SResult<SyncSettings> {
    if choice != "local" && choice != "server" {
        return Err(SyncError::value("Choose local or server."));
    }
    {
        let _guard = sync_lock();
        db::with_conn(|conn| {
            Ok((|| -> SResult<()> {
                let tx = conn.unchecked_transaction()?;
                let snap: Option<String> = tx
                    .query_row("SELECT snapshot FROM sync_conflicts WHERE recording_id=?", [recording_id], |r| r.get(0))
                    .optional()?;
                let snap = snap.ok_or_else(|| SyncError::value("Conflict not found."))?;
                let remote: Value = serde_json::from_str(&snap).map_err(|e| SyncError::value(e.to_string()))?;
                if choice == "server" {
                    apply_remote(&tx, &remote)?;
                } else {
                    tx.execute(
                        "UPDATE sync_records SET revision=?,generation=generation+1,updated_at=? WHERE recording_id=?",
                        params![get_int(&remote, "revision")?, timefmt::now_utc(), recording_id],
                    )?;
                }
                tx.execute("DELETE FROM sync_conflicts WHERE recording_id=?", [recording_id])?;
                tx.commit()?;
                Ok(())
            })())
        })??;
    }
    settings()
}

pub fn mock_scenario(scenario: &str) -> SResult<String> {
    let c = settings()?;
    if !c.mock || !c.enabled {
        return Err(SyncError::value("Enable and save mock sync first."));
    }
    if scenario == "failure" || scenario == "authentication" {
        let _guard = sync_lock();
        db::with_conn(|conn| Ok(put(conn, "mock_failure", scenario)))??;
        return Ok("Next mock sync will fail once; retry to recover.".into());
    }
    if scenario != "remote_edit" && scenario != "conflict" {
        return Err(SyncError::value("Unknown mock scenario."));
    }
    run()?;
    let _guard = sync_lock();
    let snap: Option<String> = db::with_conn(|conn| {
        conn.query_row("SELECT snapshot FROM sync_mock_records WHERE json_extract(snapshot,'$.deleted_at') IS NULL LIMIT 1", [], |r| r.get(0))
            .optional()
    })?;
    let snap = snap.ok_or_else(|| SyncError::value("Create a recording with text first."))?;
    let mut remote: Value = serde_json::from_str(&snap).map_err(|e| SyncError::value(e.to_string()))?;
    let obj = remote.as_object_mut().ok_or_else(|| SyncError::type_("snapshot"))?;
    let revision = obj.remove("revision").ok_or_else(|| SyncError::key("revision"))?;
    obj.insert("base_revision".into(), revision);
    obj.insert("mutation_id".into(), json!(uuid::Uuid::new_v4().to_string()));
    obj.insert("updated_at".into(), json!(timefmt::now_utc()));
    let filename = obj.get("filename").and_then(Value::as_str).unwrap_or("").to_owned();
    let base_title = obj
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| Path::new(&filename).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_owned());
    obj.insert("title".into(), json!(format!("{base_title} (mock device edit)")));
    let recording_id = obj.get("recording_id").and_then(Value::as_str).unwrap_or("").to_owned();
    receive(&json!({"schema_version": 1, "device_id": uuid::Uuid::new_v4().to_string(), "cursor": 0, "records": [remote]}))?;
    if scenario == "conflict" {
        let local: Option<String> = db::with_conn(|conn| {
            conn.query_row("SELECT filename FROM sync_records WHERE recording_id=?", [&recording_id], |r| r.get(0)).optional()
        })?;
        if let Some(local) = local {
            db::set_title(&local, "Local conflicting title")?;
        }
    }
    Ok("Mock second device changed a recording. Sync now to receive it.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_validation_rules() {
        assert!(valid_endpoint("https://server.example/sync"));
        assert!(!valid_endpoint("http://server.example/sync"));
        assert!(!valid_endpoint("https://user:pw@server.example/sync"));
        assert!(!valid_endpoint("https://server.example/sync?x=1"));
        assert!(!valid_endpoint("https://server.example/sync#frag"));
        assert!(!valid_endpoint("not a url"));
    }

    #[test]
    fn validates_records() {
        let good = json!({
            "recording_id": "930954bb-4292-4442-8c2b-b6b738b9dca9", "filename": "meeting.mp3",
            "created_at": "2026-10-05T14:00:00Z", "updated_at": "2026-10-05T14:35:00Z", "deleted_at": null,
            "base_revision": 0, "mutation_id": "x:1", "title": "Weekly",
            "transcription": {"text": "A: hi", "language": "en", "created_at": "2026-10-05T14:30:00Z"},
            "notes": {"id": "a7889f2e7f2741c99cf8e97e923c9dc7", "text": "n"}
        });
        validate_record(&good, false).unwrap();
        let mut bad = good.clone();
        bad["filename"] = json!("../x.mp3");
        assert_eq!(validate_record(&bad, false).unwrap_err().kind, "ValueError");
        let mut bad = good.clone();
        bad.as_object_mut().unwrap().remove("title");
        assert_eq!(validate_record(&bad, false).unwrap_err().kind, "KeyError");
        let mut bad = good;
        bad["base_revision"] = json!(true);
        assert_eq!(validate_record(&bad, false).unwrap_err().kind, "ValueError");
    }
}
