//! The application service layer: a port of the FastAPI endpoints in `app/main.py`.
//!
//! Each function returns what the endpoint returned, or an [`ApiError`] carrying the HTTP
//! status and `detail` the Python API used, so the UI shows the same messages.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::recorder::{RECORDER, RecorderError, RecorderStatus};
use crate::sync::{self, SyncError, SyncSettings};
use crate::{config, db, timefmt, titles, transcription};

pub const DATABASE_UNAVAILABLE: &str =
    "Database unavailable. Check the app's log and database permissions; requests will retry automatically.";

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{detail}")]
pub struct ApiError {
    pub status: u16,
    pub detail: String,
}

impl ApiError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        ApiError { status, detail: detail.into() }
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        eprintln!("database error: {e}");
        ApiError::new(500, "Internal Server Error")
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

// ----- Database readiness -----

struct DbState {
    ready: bool,
    retry_at: Option<Instant>,
}

static DB_STATE: Mutex<DbState> = Mutex::new(DbState { ready: false, retry_at: None });

pub fn ensure_database() -> bool {
    let mut state = DB_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.ready {
        return true;
    }
    if state.retry_at.is_some_and(|t| Instant::now() < t) {
        return false;
    }
    match db::init_db().and_then(|()| db::recover_jobs()) {
        Ok(()) => state.ready = true,
        Err(e) => {
            eprintln!("Database initialization failed; the app stays available and retries: {e}");
            state.retry_at = Some(Instant::now() + Duration::from_secs(5));
        }
    }
    // Do not retain a failed/stale startup connection between attempts.
    db::close_connection();
    state.ready
}

fn require_database() -> ApiResult<()> {
    if ensure_database() { Ok(()) } else { Err(ApiError::new(503, DATABASE_UNAVAILABLE)) }
}

/// Starts the 30-second automatic sync worker.
pub fn start_background_sync() {
    std::thread::Builder::new()
        .name("auto-sync".into())
        .spawn(|| {
            loop {
                if ensure_database() && sync::settings().is_ok_and(|s| s.enabled) {
                    // Sanitized status is persisted by the sync client.
                    let _ = run_sync();
                }
                std::thread::sleep(Duration::from_secs(30));
            }
        })
        .ok();
}

// ----- In-flight transcription jobs -----

/// filename -> "pending" | "completed" | "error"
static TRANSCRIBING: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);
/// filename -> error message
static TRANSCRIBE_ERRORS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

fn job_state(filename: &str) -> Option<String> {
    TRANSCRIBING.lock().unwrap_or_else(|e| e.into_inner()).get(filename).cloned()
}

fn set_job(filename: &str, state: &str) {
    TRANSCRIBING.lock().unwrap_or_else(|e| e.into_inner()).insert(filename.to_owned(), state.to_owned());
}

fn job_error(filename: &str) -> Option<String> {
    TRANSCRIBE_ERRORS.lock().unwrap_or_else(|e| e.into_inner()).get(filename).cloned()
}

fn set_job_error(filename: &str, error: Option<String>) {
    let mut errors = TRANSCRIBE_ERRORS.lock().unwrap_or_else(|e| e.into_inner());
    match error {
        Some(e) => errors.insert(filename.to_owned(), e),
        None => errors.remove(filename),
    };
}

fn any_pending() -> bool {
    TRANSCRIBING.lock().unwrap_or_else(|e| e.into_inner()).values().any(|v| v == "pending")
}

// ----- Schemas -----

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Settings {
    pub auto_delete_audio: bool,
    pub auto_title: bool,
    pub openrouter_configured: bool,
    pub title_model: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RecordingInfo {
    pub filename: String,
    pub name: String,
    pub size_bytes: u64,
    pub audio_available: bool,
    pub processing_error: Option<String>,
    pub processing: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct TranscriptionStatus {
    pub filename: String,
    pub title: Option<String>,
    /// "none" | "pending" | "completed" | "error"
    pub status: String,
    pub text: Option<String>,
    pub language: Option<String>,
    pub error: Option<String>,
    pub processing: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SearchResult {
    pub filename: String,
    pub title: Option<String>,
    pub snippet: String,
    pub language: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
pub struct StorageInfo {
    pub used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_free_bytes: u64,
}

// ----- Settings -----

pub fn get_settings() -> ApiResult<Settings> {
    require_database()?;
    let s = db::get_settings()?;
    Ok(Settings {
        auto_delete_audio: s.auto_delete_audio,
        auto_title: s.auto_title,
        openrouter_configured: titles::configured(),
        title_model: titles::MODEL.into(),
    })
}

pub fn update_settings(auto_delete_audio: bool, auto_title: bool) -> ApiResult<Settings> {
    require_database()?;
    db::save_settings(db::RecordingSettings { auto_delete_audio, auto_title })?;
    get_settings()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    /// Follow the Windows app mode.
    #[default]
    Auto,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::Auto, Appearance::Light, Appearance::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::Auto => "Auto",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }

    fn from_i64(v: i64) -> Self {
        match v {
            1 => Appearance::Light,
            2 => Appearance::Dark,
            _ => Appearance::Auto,
        }
    }

    fn to_i64(self) -> i64 {
        match self {
            Appearance::Auto => 0,
            Appearance::Light => 1,
            Appearance::Dark => 2,
        }
    }
}

/// The saved appearance; Auto when the database isn't available.
pub fn get_appearance() -> Appearance {
    if !ensure_database() {
        return Appearance::Auto;
    }
    db::get_appearance().map(Appearance::from_i64).unwrap_or_default()
}

pub fn set_appearance(appearance: Appearance) -> ApiResult<()> {
    require_database()?;
    Ok(db::set_appearance(appearance.to_i64())?)
}

// ----- Recordings -----

pub fn recording_path(filename: &str) -> ApiResult<PathBuf> {
    let plain = Path::new(filename).file_name().and_then(|n| n.to_str()) == Some(filename);
    if !plain || filename.contains(['/', '\\', ':']) || filename == "." || filename == ".." {
        return Err(ApiError::new(400, "Invalid recording filename"));
    }
    Ok(config::recordings_dir().join(filename))
}

fn stem(filename: &str) -> String {
    Path::new(filename).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn recording_info(filename: &str) -> ApiResult<RecordingInfo> {
    let filepath = recording_path(filename)?;
    if db::is_deleted(filename)? {
        return Err(ApiError::new(404, "Recording deleted"));
    }
    // Automatic cleanup may finish while the recordings list is loading.
    let stat = std::fs::metadata(&filepath).ok().filter(|m| m.is_file());
    let transcript = db::get_transcription(filename)?;
    let metadata = db::get_metadata(filename)?;
    if stat.is_none() && transcript.is_none() && metadata.is_none() {
        return Err(ApiError::new(404, "Recording not found"));
    }
    let created_at = match (&stat, &transcript, &metadata) {
        (Some(m), _, _) => {
            let modified: chrono::DateTime<chrono::Local> = m.modified().map(Into::into).unwrap_or_else(|_| chrono::Local::now());
            timefmt::local_aware(modified)
        }
        (None, Some(t), _) => t.created_at.clone(),
        (None, None, Some(m)) => m.created_at.clone(),
        (None, None, None) => unreachable!(),
    };
    db::ensure_metadata(filename, &created_at)?;
    let metadata = db::get_metadata(filename)?.unwrap_or_default();
    Ok(RecordingInfo {
        filename: filename.to_owned(),
        name: metadata.title.filter(|t| !t.is_empty()).unwrap_or_else(|| stem(filename)),
        size_bytes: stat.as_ref().map_or(0, std::fs::Metadata::len),
        audio_available: stat.is_some(),
        created_at: metadata.created_at,
        processing_error: metadata.processing_error.filter(|e| !e.is_empty()).or_else(|| job_error(filename)),
        processing: job_state(filename).as_deref() == Some("pending"),
    })
}

fn require_idle(filename: &str) -> ApiResult<()> {
    let busy_recording = (RECORDER.is_recording() || RECORDER.save_pending()) && RECORDER.current_file().as_deref() == Some(stem(filename).as_str());
    if job_state(filename).as_deref() == Some("pending") || busy_recording {
        return Err(ApiError::new(409, "Wait until recording and processing finish."));
    }
    Ok(())
}

pub fn get_status() -> RecorderStatus {
    RECORDER.recover_pending();
    RECORDER.status()
}

pub fn start_recording(name: Option<&str>) -> ApiResult<RecorderStatus> {
    require_database()?;
    let name = name.filter(|n| !n.is_empty());
    RECORDER.start(name).map_err(|e| match e {
        RecorderError::Invalid(m) => ApiError::new(400, m),
        RecorderError::Conflict(m) => ApiError::new(409, m),
    })?;
    Ok(get_status())
}

pub fn stop_recording() -> ApiResult<RecorderStatus> {
    require_database()?;
    let filepath = RECORDER.stop().map_err(|e| ApiError::new(409, e.to_string()))?;
    let name = filepath.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    set_job(&name, "pending");
    set_job_error(&name, None);
    if let Err(e) = recording_info(&name).and_then(|_| db::set_job_state(&name, "pending").map_err(Into::into)) {
        // Audio is already saved. Let the worker report a database failure.
        eprintln!("Could not persist queued processing job: {e}");
    }
    spawn_transcription(name.clone());
    Ok(RecorderStatus { recording: false, filename: Some(name), notes_id: RECORDER.notes_id(), ..Default::default() })
}

pub fn get_recording_notes(filename: &str) -> ApiResult<db::Notes> {
    require_database()?;
    recording_info(filename)?;
    Ok(db::create_notes(Some(filename))?)
}

pub fn get_notes(notes_id: &str) -> ApiResult<db::Notes> {
    require_database()?;
    db::get_notes(notes_id)?.ok_or_else(|| ApiError::new(404, "Notes not found"))
}

pub fn update_notes(notes_id: &str, text: &str) -> ApiResult<db::Notes> {
    require_database()?;
    if !db::save_notes(notes_id, text)? {
        return Err(ApiError::new(404, "Notes not found"));
    }
    Ok(db::Notes { id: notes_id.to_owned(), text: text.to_owned() })
}

pub fn list_recordings() -> ApiResult<Vec<RecordingInfo>> {
    require_database()?;
    let dir = config::recordings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new(500, e.to_string()))?;
    let mut filenames: HashSet<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".mp3") || n.ends_with(".wav"))
                .collect()
        })
        .unwrap_or_default();
    filenames.extend(db::list_transcription_filenames()?);
    filenames.extend(db::list_metadata_filenames()?);
    for deleted in db::deleted_filenames()? {
        filenames.remove(&deleted);
    }
    let mut recordings = filenames.iter().map(|f| recording_info(f)).collect::<ApiResult<Vec<_>>>()?;
    recordings.sort_by(|a, b| timefmt::timestamp(&b.created_at).total_cmp(&timefmt::timestamp(&a.created_at)));
    Ok(recordings)
}

#[cfg(windows)]
fn disk_usage(path: &Path) -> (u64, u64) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated and the out pointers are valid for the call.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    if ok == 0 { (0, 0) } else { (total, free) }
}

#[cfg(not(windows))]
fn disk_usage(_path: &Path) -> (u64, u64) {
    (0, 0)
}

pub fn get_storage() -> ApiResult<StorageInfo> {
    require_database()?;
    let dir = config::recordings_dir();
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::new(500, e.to_string()))?;
    let used = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0);
    let (total, free) = disk_usage(&dir);
    Ok(StorageInfo { used_bytes: used, disk_total_bytes: total, disk_free_bytes: free })
}

pub fn delete_recording(filename: &str) -> ApiResult<()> {
    require_database()?;
    require_idle(filename)?;
    recording_info(filename)?;
    let path = recording_path(filename)?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(ApiError::new(500, e.to_string())),
    }
    db::delete_transcription(filename)?;
    db::delete_metadata(filename)?;
    TRANSCRIBING.lock().unwrap_or_else(|e| e.into_inner()).remove(filename);
    set_job_error(filename, None);
    Ok(())
}

pub fn rename_recording(filename: &str, new_name: &str) -> ApiResult<RecordingInfo> {
    require_database()?;
    require_idle(filename)?;
    recording_info(filename)?;
    let title = new_name.trim();
    if title.is_empty() || title.chars().count() > 160 {
        return Err(ApiError::new(400, "Title must contain 1 to 160 characters"));
    }
    db::set_title(filename, title)?;
    recording_info(filename)
}

/// The audio file for playback or download, when it exists.
pub fn audio_file(filename: &str) -> ApiResult<PathBuf> {
    let path = recording_path(filename)?;
    if !path.is_file() {
        return Err(ApiError::new(404, "Recording not found"));
    }
    Ok(path)
}

// ----- Transcription -----

fn spawn_transcription(filename: String) {
    let spawned = std::thread::Builder::new().name("transcribe".into()).spawn({
        let filename = filename.clone();
        move || do_transcribe(&filename)
    });
    if spawned.is_err() {
        set_job(&filename, "error");
        set_job_error(&filename, Some("Could not start transcription worker".into()));
    }
}

/// Save the transcript before any destructive audio cleanup.
fn do_transcribe(filename: &str) {
    let mut transcript_saved = false;
    let mut errors: Vec<String> = Vec::new();
    let outcome = (|| -> Result<(), String> {
        let filepath = recording_path(filename).map_err(|e| e.detail)?;
        let settings = db::get_settings().map_err(|e| e.to_string())?;
        recording_info(filename).map_err(|e| e.detail)?;
        db::set_processing_error(filename, None).map_err(|e| e.to_string())?;
        db::set_job_state(filename, "pending").map_err(|e| e.to_string())?;
        let result = transcription::transcribe_file(&filepath, None)?;
        db::save_transcription(filename, &result.text, result.language.as_deref()).map_err(|e| e.to_string())?;
        transcript_saved = true;
        if settings.auto_delete_audio {
            match std::fs::remove_file(&filepath) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => errors.push("Transcript saved, but audio could not be deleted. You can remove it manually.".into()),
            }
        }
        if settings.auto_title {
            match titles::suggest_title(&result.text) {
                Ok(title) => db::set_title(filename, &title).map_err(|e| e.to_string())?,
                Err(e) => errors.push(e),
            }
        }
        let joined = errors.join(" ");
        db::set_processing_error(filename, (!joined.is_empty()).then_some(joined.as_str())).map_err(|e| e.to_string())
    })();
    if let Err(e) = outcome {
        eprintln!("Recording processing failed for {filename}: {e}");
        let prefix = if transcript_saved { "Transcript saved, but post-processing failed: " } else { "" };
        errors.push(format!("{prefix}{e}"));
    }
    let persisted = (|| -> db::DbResult<()> {
        db::ensure_metadata(filename, &timefmt::now_local_naive())?;
        let joined = errors.join(" ");
        db::set_processing_error(filename, (!joined.is_empty()).then_some(joined.as_str()))?;
        db::set_job_state(filename, if transcript_saved { "completed" } else { "error" })
    })();
    if let Err(e) = persisted {
        eprintln!("Could not persist processing outcome: {e}");
        errors.push(e.to_string());
    }
    // Finalize in memory even if the database cannot store the failure.
    set_job_error(filename, (!errors.is_empty()).then(|| errors.join(" ")));
    set_job(filename, if transcript_saved { "completed" } else { "error" });
    db::close_connection();
}

pub fn start_transcription(filename: &str) -> ApiResult<TranscriptionStatus> {
    require_database()?;
    let filepath = recording_path(filename)?;
    if db::get_transcription(filename)?.is_some() || job_state(filename).as_deref() == Some("pending") {
        return get_transcription(filename);
    }
    if !filepath.is_file() {
        return Err(ApiError::new(404, "Audio not found"));
    }
    recording_info(filename)?;
    db::set_processing_error(filename, None)?;
    db::set_job_state(filename, "pending")?;
    set_job(filename, "pending");
    set_job_error(filename, None);
    spawn_transcription(filename.to_owned());
    Ok(TranscriptionStatus { filename: filename.to_owned(), status: "pending".into(), ..Default::default() })
}

pub fn get_transcription(filename: &str) -> ApiResult<TranscriptionStatus> {
    require_database()?;
    let metadata = db::get_metadata(filename)?.unwrap_or_default();
    let state = job_state(filename);
    let processing = state.as_deref() == Some("pending");
    let error = job_error(filename).or(metadata.processing_error.filter(|e| !e.is_empty()));
    // Transcript availability is independent of post-processing.
    if let Some(existing) = db::get_transcription(filename)? {
        return Ok(TranscriptionStatus {
            filename: filename.to_owned(),
            title: metadata.title,
            processing,
            error,
            status: "completed".into(),
            text: Some(existing.text),
            language: existing.language,
        });
    }
    if processing {
        return Ok(TranscriptionStatus { filename: filename.to_owned(), status: "pending".into(), ..Default::default() });
    }
    if state.as_deref() == Some("error") || error.is_some() {
        return Ok(TranscriptionStatus {
            filename: filename.to_owned(),
            status: "error".into(),
            error: Some(error.unwrap_or_else(|| "Unknown error".into())),
            ..Default::default()
        });
    }
    Ok(TranscriptionStatus { filename: filename.to_owned(), status: "none".into(), ..Default::default() })
}

/// Update transcription text (e.g. after speaker rename).
pub fn update_transcription(filename: &str, text: &str) -> ApiResult<TranscriptionStatus> {
    require_database()?;
    if db::get_transcription(filename)?.is_none() {
        return Err(ApiError::new(404, "Transcription not found"));
    }
    db::update_transcription_text(filename, text)?;
    let updated = db::get_transcription(filename)?.ok_or_else(|| ApiError::new(404, "Transcription not found"))?;
    Ok(TranscriptionStatus {
        filename: filename.to_owned(),
        status: "completed".into(),
        text: Some(updated.text),
        language: updated.language,
        ..Default::default()
    })
}

pub fn search_transcriptions(q: &str) -> ApiResult<Vec<SearchResult>> {
    require_database()?;
    if q.trim().is_empty() {
        return Ok(Vec::new());
    }
    let titles = db::titles()?;
    Ok(db::search_transcriptions(q)?
        .into_iter()
        .map(|r| SearchResult { title: titles.get(&r.filename).cloned().flatten(), filename: r.filename, snippet: r.snippet, language: r.language })
        .collect())
}

// ----- Sync -----

fn sync_error(e: SyncError) -> ApiError {
    if e.kind == "ValueError" { ApiError::new(400, e.message) } else { ApiError::new(500, "Internal Server Error") }
}

pub fn sync_settings() -> ApiResult<SyncSettings> {
    require_database()?;
    sync::settings().map_err(sync_error)
}

pub fn update_sync_settings(enabled: bool, endpoint: &str, mock: bool, api_key: Option<&str>) -> ApiResult<SyncSettings> {
    require_database()?;
    sync::save_settings(enabled, endpoint, mock, api_key).map_err(sync_error)
}

fn require_sync_idle() -> ApiResult<()> {
    // Avoid overwriting recordings currently being processed/captured.
    if RECORDER.is_recording() || RECORDER.save_pending() || any_pending() {
        return Err(ApiError::new(409, "Wait until recording and processing finish."));
    }
    Ok(())
}

pub fn run_sync() -> ApiResult<sync::RunResult> {
    require_database()?;
    require_sync_idle()?;
    sync::run().map_err(|_| {
        let detail = sync::settings().ok().and_then(|s| s.error).unwrap_or_else(|| "Sync unavailable.".into());
        ApiError::new(400, detail)
    })
}

pub fn sync_conflicts() -> ApiResult<Vec<sync::Conflict>> {
    require_database()?;
    sync::conflicts().map_err(sync_error)
}

pub fn resolve_sync_conflict(recording_id: &str, choice: &str) -> ApiResult<SyncSettings> {
    require_database()?;
    require_sync_idle()?;
    sync::resolve(recording_id, choice).map_err(sync_error)
}

pub fn mock_scenario(scenario: &str) -> ApiResult<String> {
    require_database()?;
    sync::mock_scenario(scenario).map_err(|e| {
        if e.kind == "ValueError" { ApiError::new(400, e.message) } else { ApiError::new(400, "Mock scenario failed. Check sync status.") }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal() {
        for bad in ["../x.mp3", "a/b.mp3", "a\\b.mp3", "C:x.mp3", ".", ".."] {
            assert_eq!(recording_path(bad).unwrap_err().status, 400, "{bad}");
        }
        assert!(recording_path("meeting.mp3").is_ok());
    }

    /// End-to-end over a temporary data folder: notes, transcripts, search, settings and a
    /// mock sync round trip including a conflict.
    #[test]
    fn database_flows() {
        let dir = tempfile::tempdir().unwrap();
        config::set_data_dir(dir.path().to_path_buf());
        assert!(ensure_database());
        assert_eq!(get_settings().unwrap().auto_title, false);
        assert!(update_settings(true, false).unwrap().auto_delete_audio);
        assert_eq!(get_appearance(), Appearance::Auto);
        set_appearance(Appearance::Dark).unwrap();
        assert_eq!(get_appearance(), Appearance::Dark);
        // The other settings keep their values.
        assert!(get_settings().unwrap().auto_delete_audio);

        db::ensure_metadata("meeting.mp3", "2026-10-05T14:00:00").unwrap();
        db::save_transcription("meeting.mp3", "Speaker 1: Let's review the roadmap", Some("en")).unwrap();
        let notes = get_recording_notes("meeting.mp3").unwrap();
        update_notes(&notes.id, "follow up").unwrap();
        assert_eq!(get_notes(&notes.id).unwrap().text, "follow up");

        let results = search_transcriptions("roadmap review").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("**roadmap**"));
        assert!(search_transcriptions("\"unbalanced").unwrap().is_empty());

        let list = list_recordings().unwrap();
        assert_eq!(list[0].name, "meeting");
        assert!(!list[0].audio_available);
        assert_eq!(rename_recording("meeting.mp3", "  Roadmap  ").unwrap().name, "Roadmap");
        assert_eq!(get_transcription("meeting.mp3").unwrap().status, "completed");

        // Mock sync.
        assert_eq!(update_sync_settings(true, "", true, None).unwrap_err().status, 400);
        update_sync_settings(true, "", true, Some("test-key")).unwrap();
        let first = run_sync().unwrap();
        assert_eq!(first.uploaded, 1);
        mock_scenario("conflict").unwrap();
        run_sync().unwrap();
        let conflicts = sync_conflicts().unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].local["title"], "Local conflicting title");
        resolve_sync_conflict(&conflicts[0].recording_id, "server").unwrap();
        assert_eq!(get_transcription("meeting.mp3").unwrap().title.as_deref(), Some("Roadmap (mock device edit)"));
        mock_scenario("failure").unwrap();
        assert!(run_sync().unwrap_err().detail.contains("TimeoutError"));
        run_sync().unwrap();
        assert!(sync_settings().unwrap().error.is_none());

        delete_recording("meeting.mp3").unwrap();
        assert!(list_recordings().unwrap().is_empty());
    }
}
