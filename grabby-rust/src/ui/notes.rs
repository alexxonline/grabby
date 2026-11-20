//! Per-recording notes with autosave (`components/NotesEditor.tsx`).

use std::time::{Duration, Instant};

use rune::tok::{space, type_};
use rune::{El, TextEdit, Tx, col, row, text};
use rune_kit::{ButtonVariant, TextArea};

use super::{P, Ui, button, error, muted};
use crate::core;

#[derive(Default)]
pub struct NotesState {
    /// What the editor is bound to: a notes id or a recording filename.
    pub source: Option<Source>,
    pub id: Option<String>,
    pub edit: TextEdit,
    pub saved: String,
    pub ready: bool,
    pub status: String,
    pub error: String,
    /// When the text last changed without being saved (600 ms autosave debounce).
    pub changed_at: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Id(String),
    File(String),
}

/// Binds the editor to a recording's notes, saving the previous notes first.
pub fn open(s: &mut Ui, source: Source) {
    if s.notes.source.as_ref() == Some(&source) && s.notes.ready {
        return;
    }
    flush(s);
    s.notes = NotesState { source: Some(source), status: "Loading notes…".into(), ..Default::default() };
    load(s);
}

pub fn close(s: &mut Ui) {
    flush(s);
    s.notes = NotesState::default();
}

fn load(s: &mut Ui) {
    let Some(source) = s.notes.source.clone() else { return };
    let result = match &source {
        Source::Id(id) => core::get_notes(id),
        Source::File(filename) => core::get_recording_notes(filename),
    };
    match result {
        Ok(notes) => {
            s.notes.id = Some(notes.id);
            s.notes.edit = TextEdit::new(notes.text.clone());
            s.notes.saved = notes.text;
            s.notes.ready = true;
            s.notes.status = "Saved".into();
            s.notes.error.clear();
        }
        Err(_) => {
            s.notes.status.clear();
            s.notes.error = "Could not load notes. Please retry.".into();
        }
    }
}

/// Saves pending edits now (blur, tab switch, navigation, window close).
pub fn flush(s: &mut Ui) {
    if s.notes.ready && s.notes.edit.text != s.notes.saved {
        save(s);
    }
}

fn save(s: &mut Ui) {
    s.notes.changed_at = None;
    let Some(id) = s.notes.id.clone() else { return };
    if s.notes.edit.text == s.notes.saved {
        s.notes.status = "Saved".into();
        s.notes.error.clear();
        return;
    }
    let snapshot = s.notes.edit.text.clone();
    match core::update_notes(&id, &snapshot) {
        Ok(_) => {
            s.notes.saved = snapshot;
            s.notes.status = if s.notes.edit.text == s.notes.saved { "Saved".into() } else { "Unsaved changes".into() };
            s.notes.error.clear();
        }
        Err(_) => {
            s.notes.status = "Not saved".into();
            s.notes.error = "Could not save notes. Your edits are still here. Retry when connected.".into();
        }
    }
}

pub fn tick(s: &mut Ui) -> bool {
    match s.notes.changed_at {
        Some(t) if t.elapsed() >= Duration::from_millis(600) => {
            save(s);
            true
        }
        _ => false,
    }
}

fn on_change(s: &mut Ui, _: &mut Tx) {
    if s.notes.edit.text != s.notes.saved {
        s.notes.status = "Unsaved changes".into();
        s.notes.changed_at = Some(Instant::now());
    }
}

pub fn view(s: &Ui, p: &P) -> El {
    let n = &s.notes;
    let editor = TextArea::new(&n.edit)
        .label("Your notes")
        .placeholder("Write your notes here…")
        .rows(12)
        .disabled(!n.ready)
        .on_edit(|s: &mut Ui| &mut s.notes.edit)
        .on_change(on_change)
        .build(&p.k)
        .key("custom-notes")
        .w_full();
    let mut section = col()
        .key("notes-editor")
        .w_full()
        .gap(space::S2)
        .child(text("Your notes").key("notes-label").font(type_::TITLE).color(p.t.text))
        .child(muted(p, "notes-hint", "Add key points, questions, or action items. Notes save automatically with this recording."))
        .child(editor)
        .child(
            row()
                .key("notes-footer")
                .w_full()
                .center_y()
                .justify_between()
                .child(text(n.status.clone()).key("notes-status").font(type_::LABEL).color(p.t.text_secondary).role("status"))
                .child(button(p, "save-notes", "Save notes", ButtonVariant::Submit, !n.ready, save)),
        );
    if !n.error.is_empty() {
        section = section.child(
            row()
                .key("notes-error")
                .gap(space::S2)
                .center_y()
                .child(error(p, "notes-error-text", n.error.clone()).grow(1.0))
                .child(button(p, "notes-retry", "Retry", ButtonVariant::Outline, false, |s: &mut Ui| {
                    if s.notes.ready { save(s) } else { load(s) }
                })),
        );
    }
    section
}
