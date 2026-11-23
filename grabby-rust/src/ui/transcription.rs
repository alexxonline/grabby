use std::time::{Duration, Instant};

use rune::tok::{space, type_};
use rune::{El, Key, Span, TextEdit, Tx, col, row, text};
use rune_kit::{ButtonVariant, TextInput};

use super::notes::{self, Source};
use super::{P, Page, Ui, button, button_tx, card, error, h1, muted, navigate, recording_tabs};
use crate::core::{self, JobStatus, TranscriptionStatus};
use crate::speakers::{SpeakerEntry, extract_speakers, replace_speaker, split_label};

#[derive(Default)]
pub struct TranscriptionPage {
    pub filename: String,
    pub transcription: Option<TranscriptionStatus>,
    pub display_text: String,
    pub speakers: Vec<SpeakerEntry>,
    pub editing: Option<usize>,
    pub edit_value: TextEdit,
    pub saving: bool,
    pub copied_until: Option<Instant>,
    pub error: Option<String>,
    pub loading: bool,
    loaded_text: bool,
    last_poll: Option<Instant>,
}

pub fn enter(s: &mut Ui, filename: &str, notes_tab: bool) {
    s.tr = TranscriptionPage { filename: filename.to_owned(), loading: true, ..Default::default() };
    s.detail_tab = usize::from(notes_tab);
    notes::open(s, Source::File(filename.to_owned()));
    poll_now(s);
}

fn poll_now(s: &mut Ui) {
    s.tr.last_poll = Some(Instant::now());
    match core::get_transcription(&s.tr.filename) {
        Ok(ts) => {
            if ts.status == JobStatus::Completed && !s.tr.loaded_text {
                if let Some(text) = &ts.text {
                    s.tr.loaded_text = true;
                    s.tr.display_text = text.clone();
                    s.tr.speakers = extract_speakers(text);
                }
            }
            s.tr.transcription = Some(ts);
        }
        Err(e) => s.tr.error = Some(e.detail),
    }
    s.tr.loading = false;
}

pub fn tick(s: &mut Ui) -> bool {
    let mut changed = false;
    if s.tr.copied_until.is_some_and(|t| Instant::now() >= t) {
        s.tr.copied_until = None;
        changed = true;
    }
    let waiting = s.tr.transcription.as_ref().is_some_and(|t| t.status == JobStatus::Pending || t.processing);
    if waiting && s.tr.last_poll.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
        poll_now(s);
        changed = true;
    }
    changed
}

fn start_editing(s: &mut Ui, idx: usize) {
    s.tr.editing = Some(idx);
    s.tr.edit_value = TextEdit::new(s.tr.speakers[idx].current.clone());
}

fn cancel_editing(s: &mut Ui) {
    s.tr.editing = None;
    s.tr.edit_value = TextEdit::default();
}

fn confirm_rename(s: &mut Ui) {
    let Some(idx) = s.tr.editing else { return };
    if s.tr.saving {
        return;
    }
    let new_name = s.tr.edit_value.text.trim().to_owned();
    if new_name.is_empty() {
        cancel_editing(s);
        return;
    }
    if new_name.contains([':', '\r', '\n']) {
        s.tr.error = Some("Speaker names cannot contain colons or line breaks.".into());
        return;
    }
    if s.tr.speakers.iter().enumerate().any(|(i, sp)| i != idx && sp.current == new_name) {
        s.tr.error = Some("Choose a different name for each speaker.".into());
        return;
    }
    s.tr.error = None;
    let current = s.tr.speakers[idx].current.clone();
    if new_name == current {
        cancel_editing(s);
        return;
    }
    let updated_text = replace_speaker(&s.tr.display_text, &current, &new_name);
    s.tr.saving = true;
    match core::update_transcription(&s.tr.filename, &updated_text) {
        Ok(_) => {
            s.tr.display_text = updated_text;
            s.tr.speakers[idx].current = new_name;
            cancel_editing(s);
        }
        Err(e) => s.tr.error = Some(e.detail),
    }
    s.tr.saving = false;
}

fn copy(s: &mut Ui, tx: &mut Tx) {
    tx.copy(s.tr.display_text.clone());
    s.tr.copied_until = Some(Instant::now() + Duration::from_secs(2));
}

fn render_transcript(p: &P, body: &str) -> El {
    let mut lines = col().key("transcript-body").w_full().gap(space::S2);
    for (i, line) in body.split('\n').enumerate() {
        let el = match split_label(line) {
            Some((label, _)) => {
                text(line.to_owned()).spans(vec![Span::new(0, label.len() + 1).bold().color(p.accent)])
            }
            None => text(if line.is_empty() { "\u{00A0}".to_owned() } else { line.to_owned() }),
        };
        lines = lines.child(el.key(format!("line-{i}")).font(type_::BODY).color(p.t.text).wrap().selectable(true).selection_group(true));
    }
    lines
}

fn speaker_editor(s: &Ui, p: &P) -> El {
    let t = &s.tr;
    let mut chips = row().key("speaker-chips").gap(space::S2).flex_wrap().center_y();
    for (idx, speaker) in t.speakers.iter().enumerate() {
        let chip = if t.editing == Some(idx) {
            row()
                .key(format!("chip-{}", speaker.original))
                .gap(space::S1)
                .center_y()
                .child(
                    TextInput::new(&t.edit_value)
                        .width(Some(180.0))
                        .disabled(t.saving)
                        .on_edit(|s: &mut Ui| &mut s.tr.edit_value)
                        .on_submit(|s: &mut Ui, _| confirm_rename(s))
                        .on_key(|s: &mut Ui, ev, _| {
                            if ev.key == Key::Escape {
                                cancel_editing(s);
                                return true;
                            }
                            false
                        })
                        .build(&p.k)
                        .key("speaker-input"),
                )
                .child(button(p, "ok", "Save", ButtonVariant::Submit, t.saving, confirm_rename))
                .child(button(p, "cancel", "Cancel", ButtonVariant::Ghost, t.saving, cancel_editing))
        } else {
            button(p, &format!("chip-{}", speaker.original), speaker.current.clone(), ButtonVariant::Outline, t.saving, move |s: &mut Ui| {
                start_editing(s, idx)
            })
            .help("Click to rename")
        };
        chips = chips.child(chip);
    }
    let mut editor = col()
        .key("speaker-editor")
        .w_full()
        .gap(space::S2)
        .child(text("Speakers — click a name to rename").key("speaker-label").font(type_::LABEL).color(p.t.text_secondary))
        .child(chips);
    if t.saving {
        editor = editor.child(muted(p, "speaker-saving", "Saving…"));
    }
    editor
}

pub fn view(s: &Ui, p: &P) -> El {
    let t = &s.tr;
    if t.loading {
        return muted(p, "loading", "Loading transcription…");
    }
    let stem = match t.filename.rfind('.') {
        Some(i) if i > 0 => t.filename[..i].to_owned(),
        _ => t.filename.clone(),
    };
    let ts = t.transcription.as_ref();
    let title = ts.and_then(|x| x.title.clone()).filter(|x| !x.is_empty()).unwrap_or(stem);
    let top = row()
        .key("transcription-top-bar")
        .w_full()
        .gap(space::S3)
        .center_y()
        .child(button(p, "back", "← Back", ButtonVariant::Ghost, false, |s: &mut Ui| navigate(s, Page::Recordings)))
        .child(h1(p, "title", title).grow(1.0).min_w(0.0).wrap())
        .child(button_tx(
            p,
            "copy",
            if t.copied_until.is_some() { "✓ Copied" } else { "Copy" },
            ButtonVariant::Neutral,
            t.display_text.is_empty(),
            copy,
        ));
    let mut transcript = col().key("transcript-panel").w_full().gap(space::S3);
    if let Some(e) = &t.error {
        transcript = transcript.child(error(p, "error", e.clone()));
    }
    let status = ts.map(|x| x.status).unwrap_or_default();
    if ts.is_some_and(|x| x.processing) {
        transcript = transcript.child(muted(p, "processing", "Finishing recording title and cleanup...").role("status"));
    }
    if status == JobStatus::Completed {
        if let Some(e) = ts.and_then(|x| x.error.clone()) {
            transcript = transcript.child(error(p, "post-error", e));
        }
    } else {
        let message = match status {
            JobStatus::Pending => "The transcript is processing. You can add notes now.".to_owned(),
            JobStatus::Error => format!("Transcription failed: {}", ts.and_then(|x| x.error.clone()).unwrap_or_default()),
            _ => "No transcription available yet. You can still add notes.".to_owned(),
        };
        transcript = transcript.child(muted(p, "status-message", message));
    }
    if !t.speakers.is_empty() {
        transcript = transcript.child(speaker_editor(s, p));
    }
    if status == JobStatus::Completed {
        transcript = transcript.child(render_transcript(p, &t.display_text));
    }
    col()
        .key("transcription-page")
        .w_full()
        .gap(space::S6)
        .child(top)
        .child(card(p, "details-card").child(recording_tabs(s, p, false, transcript)))
}
