//! The recordings library (`pages/RecordingsPage.tsx`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rune::tok::{space, type_};
use rune::{El, FileDialogOptions, Key, TextEdit, Tx, col, row, text};
use rune_kit::{ButtonVariant, Progress, SegmentedControl, Spinner, TextInput};

use super::{P, Page, Ui, button, button_tx, card, error, h1, muted, navigate, notify, open_transcription, player};
use crate::core::{self, RecordingInfo, StorageInfo, TranscriptionStatus};
use crate::timefmt;

const SORT_LABELS: [&str; 3] = ["Newest", "Oldest", "Name"];
const FILTER_LABELS: [&str; 4] = ["All", "Transcribed", "Processing", "Failed"];

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Sort {
    #[default]
    Newest,
    Oldest,
    Name,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Transcribed,
    Processing,
    Failed,
}

#[derive(Default)]
pub struct RecordingsPage {
    sort: Sort,
    filter: Filter,
    pub list: Vec<RecordingInfo>,
    pub loading: bool,
    pub editing: Option<String>,
    pub edit_name: TextEdit,
    pub poll_error: Option<String>,
    pub error: Option<String>,
    pub storage: Option<StorageInfo>,
    pub transcriptions: HashMap<String, TranscriptionStatus>,
    pub confirm_delete: Option<String>,
    last_poll: Option<Instant>,
}

pub fn enter(s: &mut Ui) {
    s.recs.loading = true;
    s.recs.confirm_delete = None;
    fetch(s);
}

/// Esc closes an open delete prompt; true when there was one.
pub fn cancel_confirm(s: &mut Ui) -> bool {
    s.recs.confirm_delete.take().is_some()
}

fn fetch(s: &mut Ui) {
    match core::list_recordings() {
        Ok(list) => {
            s.recs.list = list;
            s.recs.poll_error = None;
            s.recs.error = None;
        }
        Err(e) => s.recs.error = Some(e.detail),
    }
    s.recs.loading = false;
    after_list_change(s);
}

/// Storage use and transcription states follow every list change.
fn after_list_change(s: &mut Ui) {
    if let Ok(storage) = core::get_storage() {
        s.recs.storage = Some(storage);
    }
    let missing: Vec<String> = s
        .recs
        .list
        .iter()
        .map(|r| r.filename.clone())
        .filter(|f| !s.recs.transcriptions.contains_key(f))
        .collect();
    for filename in missing {
        if let Ok(status) = core::get_transcription(&filename) {
            if status.status != "none" {
                s.recs.transcriptions.insert(filename, status);
            }
        }
    }
}

pub fn tick(s: &mut Ui) -> bool {
    if s.recs
        .last_poll
        .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
    {
        return false;
    }
    s.recs.last_poll = Some(Instant::now());
    let mut changed = false;
    if s.recs.list.iter().any(|r| r.processing) {
        match core::list_recordings() {
            Ok(list) => {
                s.recs.poll_error = None;
                let playing = s.player.as_ref().map(|p| p.filename.clone());
                if let Some(f) = playing {
                    if !list.iter().any(|r| r.filename == f && r.audio_available) {
                        s.player = None;
                    }
                }
                if list != s.recs.list {
                    s.recs.list = list;
                    after_list_change(s);
                }
            }
            Err(e) => s.recs.poll_error = Some(e.detail),
        }
        changed = true;
    }
    let polling: Vec<String> = s
        .recs
        .transcriptions
        .iter()
        .filter(|(_, t)| t.status == "pending" || t.processing)
        .map(|(f, _)| f.clone())
        .collect();
    for filename in polling {
        // Retry transient failures on the next interval.
        if let Ok(updated) = core::get_transcription(&filename) {
            if s.recs.transcriptions.get(&filename) != Some(&updated) {
                s.recs.transcriptions.insert(filename, updated);
                changed = true;
            }
        }
    }
    changed
}

fn handle_transcribe(s: &mut Ui, filename: &str) {
    match core::start_transcription(filename) {
        Ok(status) => {
            let completed = status.status == "completed";
            s.recs.transcriptions.insert(filename.to_owned(), status);
            fetch(s);
            if completed {
                open_transcription(s, filename, false);
            }
        }
        Err(e) => s.recs.error = Some(e.detail),
    }
}

fn handle_delete(s: &mut Ui, filename: &str) {
    s.recs.confirm_delete = None;
    match core::delete_recording(filename) {
        Ok(()) => {
            if s.player.as_ref().is_some_and(|p| p.filename == filename) {
                s.player = None;
            }
            s.recs.list.retain(|r| r.filename != filename);
            s.recs.transcriptions.remove(filename);
            after_list_change(s);
            notify(s, "Recording deleted");
        }
        Err(e) => s.recs.error = Some(e.detail),
    }
}

fn rename_confirm(s: &mut Ui) {
    let Some(filename) = s.recs.editing.clone() else {
        return;
    };
    match core::rename_recording(&filename, &s.recs.edit_name.text) {
        Ok(updated) => {
            if let Some(r) = s.recs.list.iter_mut().find(|r| r.filename == filename) {
                *r = updated;
            }
            s.recs.editing = None;
            s.recs.edit_name = TextEdit::default();
            notify(s, "Recording renamed");
        }
        Err(e) => s.recs.error = Some(e.detail),
    }
}

fn rename_cancel(s: &mut Ui) {
    s.recs.editing = None;
    s.recs.edit_name = TextEdit::default();
}

fn download(s: &mut Ui, tx: &mut Tx, filename: String) {
    let source = match core::audio_file(&filename) {
        Ok(p) => p,
        Err(e) => {
            s.recs.error = Some(e.detail);
            return;
        }
    };
    let options = FileDialogOptions {
        title: "Save recording".into(),
        file_name: Some(filename),
        ..Default::default()
    };
    tx.save_file(options, move |s: &mut Ui, result, _| match result {
        Ok(Some(target)) => {
            if let Err(e) = std::fs::copy(&source, &target) {
                s.recs.error = Some(format!("Could not save the download: {e}"));
            } else {
                notify(s, "Recording downloaded");
            }
        }
        Ok(None) => {}
        Err(e) => s.recs.error = Some(e.to_string()),
    });
}

pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1024f64.powi(3) {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", b / 1024f64.powi(3))
    }
}

fn transcribe_button(s: &Ui, p: &P, rec: &RecordingInfo) -> El {
    let filename = rec.filename.clone();
    match s
        .recs
        .transcriptions
        .get(&rec.filename)
        .map(|t| t.status.as_str())
    {
        Some("pending") => row()
            .key("transcribing")
            .gap(space::S2)
            .center_y()
            .help("Transcribing…")
            .child(Spinner::new().build(&p.k).key("spinner"))
            .child(
                text("Transcribing…")
                    .key("label")
                    .font(type_::LABEL)
                    .color(p.t.text_secondary),
            ),
        Some("completed") => button(
            p,
            "transcript",
            "Transcript",
            ButtonVariant::Outline,
            false,
            move |s: &mut Ui| open_transcription(s, &filename, false),
        ),
        Some("error") => {
            let message = s
                .recs
                .transcriptions
                .get(&rec.filename)
                .and_then(|t| t.error.clone())
                .unwrap_or_default();
            button(
                p,
                "retry",
                "Retry",
                ButtonVariant::Warning,
                false,
                move |s: &mut Ui| handle_transcribe(s, &filename),
            )
            .help(format!("Error: {message}. Click to retry."))
        }
        _ => button(
            p,
            "transcribe",
            "Transcribe",
            ButtonVariant::Outline,
            false,
            move |s: &mut Ui| handle_transcribe(s, &filename),
        ),
    }
}

fn recording_item(s: &Ui, p: &P, rec: &RecordingInfo) -> El {
    let filename = rec.filename.clone();
    let info = if s.recs.editing.as_deref() == Some(rec.filename.as_str()) {
        row()
            .key("rename-form")
            .gap(space::S2)
            .center_y()
            .child(
                TextInput::new(&s.recs.edit_name)
                    .width(Some(260.0))
                    .on_edit(|s: &mut Ui| &mut s.recs.edit_name)
                    .on_submit(|s: &mut Ui, _| rename_confirm(s))
                    .on_key(|s: &mut Ui, ev, _| {
                        if ev.key == Key::Escape {
                            rename_cancel(s);
                            return true;
                        }
                        false
                    })
                    .build(&p.k)
                    .key("rename-input"),
            )
            .child(button(
                p,
                "rename-save",
                "Save",
                ButtonVariant::Submit,
                false,
                rename_confirm,
            ))
            .child(button(
                p,
                "rename-cancel",
                "Cancel",
                ButtonVariant::Ghost,
                false,
                rename_cancel,
            ))
    } else {
        text(rec.name.clone())
            .key("recording-name")
            .font(type_::TITLE)
            .color(p.t.text)
            .wrap()
    };
    let size = if rec.audio_available {
        format_size(rec.size_bytes)
    } else {
        "Transcript only".into()
    };
    let meta = text(format!(
        "{}  •  {}",
        timefmt::display(&rec.created_at),
        size
    ))
    .key("recording-meta")
    .font(type_::LABEL)
    .color(p.t.text_secondary);
    let playing = s
        .player
        .as_ref()
        .is_some_and(|pl| pl.filename == rec.filename);
    let mut actions = row()
        .key("recording-actions")
        .gap(space::S2)
        .flex_wrap()
        .center_y();
    let notes_file = filename.clone();
    actions = actions.child(button(
        p,
        "notes",
        "Notes",
        ButtonVariant::Ghost,
        false,
        move |s: &mut Ui| open_transcription(s, &notes_file, true),
    ));
    if rec.audio_available {
        let (f, title) = (filename.clone(), rec.name.clone());
        actions = actions.child(button(
            p,
            "play",
            if playing { "Stop" } else { "Play" },
            ButtonVariant::Neutral,
            false,
            move |s: &mut Ui| player::toggle_for(s, &f, &title),
        ));
    }
    actions = actions.child(transcribe_button(s, p, rec));
    if rec.audio_available {
        let f = filename.clone();
        actions = actions.child(button_tx(
            p,
            "download",
            "Download",
            ButtonVariant::Ghost,
            false,
            move |s: &mut Ui, tx| download(s, tx, f.clone()),
        ));
    }
    let (f, name) = (filename.clone(), rec.name.clone());
    actions = actions.child(button(
        p,
        "rename",
        "Rename",
        ButtonVariant::Ghost,
        s.recs.editing.as_deref() == Some(rec.filename.as_str()) || rec.processing,
        move |s: &mut Ui| {
            s.recs.editing = Some(f.clone());
            s.recs.edit_name = TextEdit::new(name.clone());
        },
    ));
    let f = filename.clone();
    actions = actions.child(button(
        p,
        "delete",
        "Delete",
        ButtonVariant::Danger,
        rec.processing,
        move |s: &mut Ui| s.recs.confirm_delete = Some(f.clone()),
    ));
    let mut item = col()
        .key(format!("rec-{}", rec.filename))
        .w_full()
        .padding(space::S3)
        .gap(space::S2)
        .radius(8.0)
        .fill(p.t.card)
        .border(1.0, p.t.card_border)
        .child(
            col()
                .key("recording-info")
                .gap(space::S1)
                .child(info)
                .child(meta),
        )
        .child(actions);
    if s.recs.confirm_delete.as_deref() == Some(rec.filename.as_str()) {
        let f = filename.clone();
        item = item.child(
            row()
                .key("confirm-delete")
                .gap(space::S2)
                .center_y()
                .flex_wrap()
                .role("alertdialog")
                .child(
                    text(format!("Delete \"{}\"?", rec.filename))
                        .key("confirm-text")
                        .font(type_::BODY)
                        .color(p.t.text),
                )
                .child(button(
                    p,
                    "confirm-yes",
                    "Delete",
                    ButtonVariant::Danger,
                    false,
                    move |s: &mut Ui| handle_delete(s, &f),
                ))
                .child(button(
                    p,
                    "confirm-no",
                    "Cancel",
                    ButtonVariant::Ghost,
                    false,
                    |s: &mut Ui| s.recs.confirm_delete = None,
                )),
        );
    }
    if rec.processing {
        let label = match &rec.stage {
            Some(stage) => format!("{stage}…"),
            None => "Processing recording…".to_owned(),
        };
        item = item.child(
            row()
                .key("processing")
                .gap(space::S2)
                .center_y()
                .child(Spinner::new().build(&p.k).key("processing-spinner"))
                .child(muted(p, "processing-text", label)),
        );
    }
    if let Some(e) = &rec.processing_error {
        item = item.child(error(p, "processing-error", e.clone()));
    }
    item
}

fn matches_filter(r: &RecordingsPage, rec: &RecordingInfo) -> bool {
    let status = r
        .transcriptions
        .get(&rec.filename)
        .map(|t| t.status.as_str());
    match r.filter {
        Filter::All => true,
        Filter::Transcribed => status == Some("completed"),
        Filter::Processing => rec.processing || status == Some("pending"),
        Filter::Failed => status == Some("error") || rec.processing_error.is_some(),
    }
}

fn visible(r: &RecordingsPage) -> Vec<&RecordingInfo> {
    let mut list: Vec<&RecordingInfo> =
        r.list.iter().filter(|rec| matches_filter(r, rec)).collect();
    match r.sort {
        Sort::Newest => list.sort_by(|a, b| {
            timefmt::timestamp(&b.created_at).total_cmp(&timefmt::timestamp(&a.created_at))
        }),
        Sort::Oldest => list.sort_by(|a, b| {
            timefmt::timestamp(&a.created_at).total_cmp(&timefmt::timestamp(&b.created_at))
        }),
        Sort::Name => list.sort_by_key(|rec| rec.name.to_lowercase()),
    }
    list
}

fn sort_filter_bar(r: &RecordingsPage, p: &P) -> El {
    let sort = SegmentedControl::new(&SORT_LABELS, Some(r.sort as usize))
        .fit()
        .on_pick(|s: &mut Ui, index, _| {
            s.recs.sort = [Sort::Newest, Sort::Oldest, Sort::Name][index.min(2)]
        })
        .build(&p.k)
        .key("sort-control");
    let filter = SegmentedControl::new(&FILTER_LABELS, Some(r.filter as usize))
        .fit()
        .on_pick(|s: &mut Ui, index, _| {
            s.recs.filter = [
                Filter::All,
                Filter::Transcribed,
                Filter::Processing,
                Filter::Failed,
            ][index.min(3)]
        })
        .build(&p.k)
        .key("filter-control");
    row()
        .key("sort-filter")
        .w_full()
        .gap(space::S3)
        .flex_wrap()
        .center_y()
        .child(muted(p, "sort-label", "Sort"))
        .child(sort)
        .child(muted(p, "filter-label", "Show"))
        .child(filter)
}

pub fn view(s: &Ui, p: &P) -> El {
    let r = &s.recs;
    let mut page = col()
        .key("recordings-page")
        .w_full()
        .gap(space::S3)
        .child(h1(p, "title", "Recordings"));
    if let Some(storage) = r.storage {
        let total = storage.used_bytes + storage.disk_free_bytes;
        let fraction = if total == 0 {
            0.0
        } else {
            (storage.used_bytes as f64 / total as f64).min(1.0) as f32
        };
        page = page.child(
            card(p, "storage-bar")
                .gap(space::S2)
                .padding(space::S3)
                .child(
                    row()
                        .key("storage-label")
                        .w_full()
                        .justify_between()
                        .child(muted(
                            p,
                            "used",
                            format!(
                                "Storage used by recordings: {}",
                                format_size(storage.used_bytes)
                            ),
                        ))
                        .child(muted(
                            p,
                            "free",
                            format!("{} free on disk", format_size(storage.disk_free_bytes)),
                        )),
                )
                .child(
                    Progress::new(Some(fraction))
                        .build(&p.k)
                        .key("storage-progress")
                        .w_full()
                        .label("Recordings storage"),
                ),
        );
    }
    if let Some(e) = &r.error {
        page = page.child(error(p, "error", e.clone()));
    }
    if let Some(e) = &r.poll_error {
        page = page.child(error(p, "poll-error", e.clone()));
    }
    if r.loading {
        page = page.child(muted(p, "loading", "Loading recordings…"));
    } else if r.list.is_empty() {
        page = page.child(
            card(p, "empty")
                .items_center()
                .child(super::h2(p, "empty-title", "No recordings yet"))
                .child(muted(
                    p,
                    "empty-hint",
                    "Record a meeting and it will show up here with its transcript. Press Ctrl+R to start.",
                ))
                .child(button(
                    p,
                    "empty-start",
                    "Go to Recorder",
                    ButtonVariant::Submit,
                    false,
                    |s: &mut Ui| navigate(s, Page::Recorder),
                )),
        );
    } else {
        page = page.child(sort_filter_bar(r, p));
        let shown = visible(r);
        if shown.is_empty() {
            page = page.child(muted(p, "no-matches", "No recordings match this filter."));
        }
        let mut list = col().key("recordings-list").w_full().gap(space::S2);
        for rec in shown {
            list = list.child(recording_item(s, p, rec));
        }
        page = page.child(list);
    }
    page
}
