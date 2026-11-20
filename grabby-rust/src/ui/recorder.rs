//! The recorder (`pages/RecorderPage.tsx`).

use std::time::{Duration, Instant};

use rune::tok::{space, type_};
use rune::{El, ScrollSpec, Span, TextEdit, TextStyle, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, TextInput};

use super::notes::{self, Source};
use super::{P, Page, Ui, card, error, h2, link, muted, navigate, notify, recording_tabs, spawn_task};
use crate::core;
use crate::recorder::RecorderStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Starting,
    Stopping,
}

#[derive(Default)]
pub struct RecorderPage {
    pub status: RecorderStatus,
    pub name: TextEdit,
    pub error: Option<String>,
    pub operation: Option<Operation>,
    pub saved_file: Option<String>,
    /// Length of the recording just stopped, in seconds.
    pub saved_duration: Option<u64>,
    last_poll: Option<Instant>,
    elapsed: u64,
}

pub fn enter(s: &mut Ui) {
    // The "saved" message only belongs to the visit right after stopping.
    s.rec.saved_file = None;
    s.rec.saved_duration = None;
    s.rec.last_poll = None;
    tick(s);
}

fn elapsed_of(status: &RecorderStatus) -> u64 {
    if !status.recording {
        return 0;
    }
    let Some(started) = status
        .started_at
        .as_deref()
        .and_then(|v| chrono::DateTime::parse_from_str(v, "%Y-%m-%dT%H:%M:%S%.f%:z").ok())
    else {
        return 0;
    };
    (chrono::Local::now().timestamp_millis() - started.timestamp_millis()).max(0) as u64 / 1000
}

pub fn tick(s: &mut Ui) -> bool {
    let mut changed = false;
    if s.rec
        .last_poll
        .is_none_or(|t| t.elapsed() >= Duration::from_millis(500))
    {
        s.rec.last_poll = Some(Instant::now());
        let status = core::get_status();
        if status != s.rec.status {
            s.rec.status = status;
            changed = true;
        }
    }
    let elapsed = elapsed_of(&s.rec.status);
    if elapsed != s.rec.elapsed {
        s.rec.elapsed = elapsed;
        changed = true;
    }
    // Keep finished details only for this visit, not from a stale polled notes ID.
    let st = &s.rec.status;
    let show_details = st.notes_id.is_some()
        && (st.recording || st.save_pending || st.saving || s.rec.saved_file.is_some());
    match (&st.notes_id, show_details) {
        (Some(id), true) if s.notes.source.as_ref() != Some(&Source::Id(id.clone())) => {
            notes::open(s, Source::Id(id.clone()));
            changed = true;
        }
        (_, false) if s.notes.source.is_some() && s.page == Page::Recorder => {
            notes::close(s);
            changed = true;
        }
        _ => {}
    }
    changed
}

fn format_time(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

fn handle_start(s: &mut Ui) {
    s.rec.error = None;
    s.rec.operation = Some(Operation::Starting);
    let name = s.rec.name.text.clone();
    spawn_task(
        move || core::start_recording((!name.is_empty()).then_some(name.as_str())),
        |s, result| {
            s.rec.operation = None;
            match result {
                Ok(status) => {
                    s.rec.status = status;
                    s.rec.name = TextEdit::default();
                    s.rec.saved_file = None;
                    s.rec.saved_duration = None;
                    s.detail_tab = 0;
                }
                Err(e) => s.rec.error = Some(e.detail),
            }
        },
    );
}

fn handle_discard(s: &mut Ui) {
    s.rec.error = None;
    s.rec.status.error = None;
    s.rec.operation = Some(Operation::Stopping);
    spawn_task(core::discard_recording, |s, result| {
        s.rec.operation = None;
        match result {
            Ok(status) => {
                s.rec.saved_file = None;
                s.rec.status = status;
                notify(s, "Recording discarded");
            }
            Err(e) => s.rec.error = Some(e.detail),
        }
    });
}

/// Ctrl+R: start, stop or retry the save, like the main button.
pub fn toggle(s: &mut Ui) {
    if s.page != Page::Recorder {
        navigate(s, Page::Recorder);
    }
    if s.rec.operation.is_some() || s.rec.status.saving {
        return;
    }
    if s.rec.status.recording || s.rec.status.save_pending {
        handle_stop(s);
    } else {
        handle_start(s);
    }
}

fn handle_stop(s: &mut Ui) {
    s.rec.error = None;
    s.rec.status.error = None;
    // Only a live recording has a known length; a recovered one does not.
    let elapsed = elapsed_of(&s.rec.status);
    s.rec.saved_duration = (s.rec.status.recording && elapsed > 0).then_some(elapsed);
    s.rec.operation = Some(Operation::Stopping);
    spawn_task(core::stop_recording, |s, result| {
        s.rec.operation = None;
        match result {
            Ok(status) => {
                s.rec.saved_file = status.filename.clone();
                s.rec.status = status;
                notify(s, "Recording saved");
            }
            Err(e) => {
                s.rec.saved_duration = None;
                s.rec.error = Some(e.detail);
            }
        }
    });
}

fn source_label(source: &str) -> &'static str {
    if source == "you" { "You" } else { "Others" }
}

/// "You: text" with the label bold.
fn labelled(key: String, label: &str, body: &str, color: rune::Color) -> El {
    let line = format!("{label}: {body}");
    let bold = label.len() + 1;
    text(line)
        .key(key)
        .font(type_::BODY)
        .color(color)
        .wrap()
        .spans(vec![Span::new(0, bold).bold()])
}

fn live_transcript(s: &Ui, p: &P) -> El {
    let live = s.rec.status.live.clone().unwrap_or_default();
    let mut section = col()
        .key("live-transcript")
        .w_full()
        .gap(space::S2)
        .child(h2(p, "live-title", "Live transcript"))
        .child(muted(p, "live-hint", "Preview only. The saved transcript is generated with speaker diarization after recording."))
        .child(muted(p, "live-legend", "You = microphone · Others = system audio"));
    for source in crate::realtime::SOURCES {
        let state = live.sources.get(source);
        let (message, failed) = match state {
            Some(st) => (
                st.error.clone().unwrap_or_else(|| st.status.clone()),
                st.error.is_some(),
            ),
            None => ("connecting".to_owned(), false),
        };
        section = section.child(labelled(
            format!("source-{source}"),
            source_label(source),
            &message,
            if failed { p.danger } else { p.t.text },
        ));
    }
    let mut log = col()
        .key("live-log")
        .w_full()
        .gap(space::S2)
        .role("log")
        .label("Live transcript");
    let partials: Vec<(&str, String)> = crate::realtime::SOURCES
        .iter()
        .filter_map(|&src| {
            live.sources
                .get(src)
                .filter(|st| !st.partial.is_empty())
                .map(|st| (src, st.partial.clone()))
        })
        .collect();
    if live.segments.is_empty() && partials.is_empty() {
        log = log.child(muted(p, "waiting", "Waiting for speech…"));
    }
    for (i, segment) in live.segments.iter().enumerate() {
        log = log.child(labelled(
            format!("seg-{i}"),
            source_label(&segment.source),
            &segment.text,
            p.t.text,
        ));
    }
    for (src, partial) in partials {
        log = log.child(labelled(
            format!("partial-{src}"),
            source_label(src),
            &partial,
            p.t.text_secondary,
        ));
    }
    section.child(
        col()
            .key("live-scroll")
            .w_full()
            .h(260.0)
            .padding(space::S3)
            .radius(8.0)
            .fill(p.t.control_track)
            .scroll(ScrollSpec { stick_bottom: true })
            .child(log),
    )
}

fn saved_text(r: &RecorderPage) -> String {
    let name = r.saved_file.as_deref().map(|f| {
        std::path::Path::new(f)
            .file_stem()
            .map_or_else(|| f.to_owned(), |n| n.to_string_lossy().into_owned())
    });
    let mut text = match name {
        Some(n) => format!("Saved \"{n}\""),
        None => "Recording saved".to_owned(),
    };
    if let Some(d) = r.saved_duration {
        text.push_str(&format!(" ({})", format_time(d)));
    }
    text.push_str(". The final transcript with speaker diarization is processing.");
    text
}

pub fn view(s: &Ui, p: &P) -> El {
    let r = &s.rec;
    let st = &r.status;
    let stopping = r.operation == Some(Operation::Stopping) || st.saving;
    let badge = if stopping {
        "Saving recording…"
    } else if r.operation == Some(Operation::Starting) {
        "Starting…"
    } else if st.recording {
        "Recording"
    } else if st.save_pending {
        "Save required"
    } else {
        "Ready"
    };
    let dot = if st.recording {
        p.danger
    } else {
        p.t.text_secondary
    };
    let mut recorder_card = card(p, "recorder-card")
        .gap(space::S3)
        .items_center()
        .child(
            row()
                .key("status-badge")
                .gap(space::S2)
                .center_y()
                .px(space::S3)
                .h(28.0)
                .radius(14.0)
                .fill(p.t.control_track)
                .role("status")
                .child(
                    el().key("status-dot")
                        .size(10.0, 10.0)
                        .radius(5.0)
                        .fill(dot),
                )
                .child(
                    text(badge)
                        .key("status-text")
                        .font(type_::LABEL)
                        .color(p.t.text),
                ),
        );
    if st.recording {
        recorder_card = recorder_card.child(
            text(format_time(r.elapsed))
                .key("timer")
                .font(TextStyle::new(40.0, 300.0).mono())
                .color(p.t.text),
        );
    }
    if !st.recording && !st.save_pending && r.operation.is_none() {
        recorder_card = recorder_card.child(
            TextInput::new(&r.name)
                .placeholder("Recording name (optional)")
                .width(Some(360.0))
                .on_edit(|s: &mut Ui| &mut s.rec.name)
                .on_submit(|s: &mut Ui, _| handle_start(s))
                .build(&p.k)
                .key("name-input"),
        );
    }
    if st.recording {
        if let Some(f) = &st.filename {
            recorder_card = recorder_card.child(muted(p, "current-file", format!("File: {f}.mp3")));
        }
    }
    let busy = r.operation.is_some() || st.saving;
    let (label, variant) = if stopping {
        ("Saving…", ButtonVariant::Submit)
    } else if r.operation == Some(Operation::Starting) {
        ("Starting…", ButtonVariant::Submit)
    } else if st.recording {
        ("Stop Recording", ButtonVariant::Submit)
    } else if st.save_pending {
        ("Retry Save", ButtonVariant::Submit)
    } else {
        ("Start Recording", ButtonVariant::Submit)
    };
    let action = st.recording || st.save_pending;
    recorder_card = recorder_card.child(
        Button::new(label)
            .variant(variant)
            .control_size(ControlSize::Large)
            .disabled(busy)
            .on_press(move |s: &mut Ui| {
                if action {
                    handle_stop(s)
                } else {
                    handle_start(s)
                }
            })
            .build(&p.k)
            .key("record-button"),
    );
    if st.save_pending && !st.recording && !busy {
        recorder_card = recorder_card.child(
            Button::new("Discard Recording")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Large)
                .on_press(|s: &mut Ui| handle_discard(s))
                .build(&p.k)
                .key("discard-button"),
        );
    }
    if let Some(e) = r.error.as_ref().or(st.error.as_ref()) {
        recorder_card = recorder_card.child(error(p, "recorder-error", e.clone()));
    }
    if r.saved_file.is_some() && !st.recording {
        recorder_card = recorder_card.child(
            row()
                .key("saved")
                .gap(space::S2)
                .flex_wrap()
                .child(muted(p, "saved-text", saved_text(r)))
                .child(link(p, "saved-link", "View recordings", |s: &mut Ui| {
                    navigate(s, Page::Recordings)
                })),
        );
    }
    let mut page = col()
        .key("recorder-page")
        .w_full()
        .gap(space::S6)
        .child(recorder_card);
    let show_details = st.notes_id.is_some()
        && (st.recording || st.save_pending || st.saving || r.saved_file.is_some());
    if show_details {
        let transcript = if st.recording {
            live_transcript(s, p)
        } else {
            row()
                .key("finished")
                .gap(space::S2)
                .flex_wrap()
                .child(muted(p, "finished-text", "Recording finished."))
                .child(link(
                    p,
                    "finished-link",
                    "View the final transcript in Recordings.",
                    |s: &mut Ui| navigate(s, Page::Recordings),
                ))
        };
        page = page.child(card(p, "details-card").child(recording_tabs(
            s,
            p,
            st.recording,
            transcript,
        )));
    }
    page
}
