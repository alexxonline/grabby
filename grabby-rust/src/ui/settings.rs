use std::time::{Duration, Instant};

use rune::tok::{motion, space, type_};
use rune::{El, TextEdit, col, el, row, text};
use rune_kit::{ButtonVariant, SegmentedControl, TextInput};
use serde_json::Value;

use super::{P, Ui, button, card, error, h1, h2, muted, spawn_task};
use crate::core::{self, ApiResult, Appearance, Settings};
use crate::sync::{Conflict, SyncSettings};
use crate::timefmt;

#[derive(Default)]
pub struct SettingsPage {
    pub settings: Option<Settings>,
    pub saving: bool,
    pub error: String,
    pub saved: bool,
}

#[derive(Default)]
pub struct SyncPanel {
    pub settings: Option<SyncSettings>,
    pub endpoint: TextEdit,
    pub key: TextEdit,
    pub enabled: bool,
    pub mock: bool,
    pub busy: bool,
    pub error: String,
    pub message: String,
    pub conflicts: Vec<Conflict>,
    last_poll: Option<Instant>,
}

pub fn enter(s: &mut Ui) {
    s.settings.saved = false;
    super::keys::enter(s);
    match core::get_settings() {
        Ok(settings) => {
            s.settings.settings = Some(settings);
            s.settings.error.clear();
        }
        Err(e) => s.settings.error = e.detail,
    }
    s.sync.message.clear();
    s.sync.error.clear();
    match core::sync_settings() {
        Ok(st) => {
            s.sync.endpoint = TextEdit::new(st.endpoint.clone());
            s.sync.enabled = st.enabled;
            s.sync.mock = st.mock;
            s.sync.settings = Some(st);
        }
        Err(e) => s.sync.error = e.detail,
    }
    match core::sync_conflicts() {
        Ok(c) => s.sync.conflicts = c,
        Err(e) => s.sync.error = e.detail,
    }
    s.sync.last_poll = Some(Instant::now());
}

pub fn tick(s: &mut Ui) -> bool {
    if s.sync.busy || s.sync.last_poll.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
        return false;
    }
    s.sync.last_poll = Some(Instant::now());
    if let (Ok(st), Ok(c)) = (core::sync_settings(), core::sync_conflicts()) {
        let changed = s.sync.settings.as_ref() != Some(&st) || s.sync.conflicts != c;
        s.sync.settings = Some(st);
        s.sync.conflicts = c;
        return changed;
    }
    false
}

fn toggle(s: &mut Ui, auto_delete: bool) {
    let Some(current) = s.settings.settings.clone() else { return };
    s.settings.saving = true;
    s.settings.saved = false;
    s.settings.error.clear();
    let (a, t) = if auto_delete { (!current.auto_delete_audio, current.auto_title) } else { (current.auto_delete_audio, !current.auto_title) };
    match core::update_settings(a, t) {
        Ok(settings) => {
            s.settings.settings = Some(settings);
            s.settings.saved = true;
        }
        Err(e) => s.settings.error = if e.detail.is_empty() { "Could not save settings.".into() } else { e.detail },
    }
    s.settings.saving = false;
}

fn act(s: &mut Ui, operation: impl FnOnce() -> ApiResult<Option<String>> + Send + 'static, success: &'static str) {
    s.sync.busy = true;
    s.sync.error.clear();
    s.sync.message.clear();
    spawn_task(
        move || {
            let result = operation();
            let refreshed = (core::sync_settings(), core::sync_conflicts());
            (result, refreshed)
        },
        move |s, (result, refreshed)| {
            s.sync.busy = false;
            super::keys::refresh_status(s);
            match result {
                Ok(message) => {
                    if let (Ok(st), Ok(c)) = refreshed {
                        s.sync.settings = Some(st);
                        s.sync.conflicts = c;
                    }
                    s.sync.message = message.unwrap_or_else(|| success.to_owned());
                }
                Err(e) => {
                    if let (Ok(st), Ok(c)) = refreshed {
                        s.sync.settings = Some(st);
                        s.sync.conflicts = c;
                    }
                    s.sync.error = if e.detail.is_empty() { "Sync failed.".into() } else { e.detail };
                }
            }
        },
    );
}

fn save_sync(s: &mut Ui) {
    let (enabled, mock) = (s.sync.enabled, s.sync.mock);
    let endpoint = s.sync.endpoint.text.clone();
    let key = s.sync.key.text.clone();
    s.sync.key = TextEdit::default();
    act(
        s,
        move || core::update_sync_settings(enabled, &endpoint, mock, (!key.is_empty()).then_some(key.as_str())).map(|_| None),
        "Sync settings saved.",
    );
}

fn dirty(s: &Ui) -> bool {
    let sy = &s.sync;
    sy.settings.as_ref().is_some_and(|st| {
        sy.enabled != st.enabled || sy.endpoint.text != st.endpoint || sy.mock != st.mock || !sy.key.text.is_empty()
    })
}

fn version(p: &P, key: &str, label: &str, value: &Value) -> El {
    let deleted = !value["deleted_at"].is_null();
    let title = if deleted {
        "Deleted recording".to_owned()
    } else {
        value["title"].as_str().filter(|t| !t.is_empty()).or(value["filename"].as_str()).unwrap_or("").to_owned()
    };
    let transcript = value["transcription"]["text"].as_str().filter(|t| !t.is_empty()).unwrap_or("No transcript").to_owned();
    let notes = value["notes"]["text"].as_str().filter(|t| !t.is_empty()).unwrap_or("No notes").to_owned();
    col()
        .key(key)
        .grow(1.0)
        .min_w(0.0)
        .gap(space::S1)
        .padding(space::S3)
        .radius(8.0)
        .fill(p.t.control_track)
        .child(text(label).key("label").font(type_::TITLE).color(p.t.text))
        .child(text(title).key("title").font(type_::BODY).color(p.t.text).wrap())
        .child(text(transcript).key("transcript").font(type_::MONO_CAPTION).color(p.t.text_secondary).wrap().max_lines(8))
        .child(muted(p, "notes", format!("Notes: {notes}")))
}

fn switch_row(p: &P, key: &str, label: &str, description: &str, checked: bool, disabled: bool, f: impl Fn(&mut Ui) + 'static) -> El {
    row()
        .key(key)
        .w_full()
        .gap(space::S6)
        .center_y()
        .justify_between()
        .child(
            col()
                .key("text")
                .grow(1.0)
                .min_w(0.0)
                .gap(space::S1)
                .child(text(label).key("label").font(type_::TITLE).color(p.t.text))
                .child(muted(p, "description", description)),
        )
        .child(switch_control(p, label, checked, disabled, f))
}

fn switch_control(p: &P, label: &str, on: bool, disabled: bool, f: impl Fn(&mut Ui) + 'static) -> El {
    let (w, h, knob) = (40.0, 22.0, 18.0);
    let inset = (h - knob) / 2.0;
    let switch = el()
        .key("switch")
        .role("switch")
        .label(label)
        .checked(on)
        .focusable(!disabled)
        .disabled(disabled)
        .size(w, h)
        .radius(h / 2.0)
        .fill(if on { p.accent } else { p.t.control_off })
        .motion(motion::SNAPPY)
        .opacity(if disabled { 0.5 } else { 1.0 })
        .child(
            el().key("knob")
                .size(knob, knob)
                .absolute(if on { w - knob - inset } else { inset }, inset)
                .radius(knob / 2.0)
                .fill(p.t.knob)
                .shadow(1.0, 3.0, p.t.knob_shadow)
                .shadow(0.0, 0.5, p.t.edge)
                .a11y_hidden(),
        );
    if disabled { switch } else { switch.on_press(f) }
}

fn sync_panel(s: &Ui, p: &P) -> El {
    let sy = &s.sync;
    let mut panel = card(p, "sync-card")
        .child(h2(p, "sync-title", "Text sync"))
        .child(muted(
            p,
            "sync-intro",
            "Sync titles, transcripts and notes across devices. Audio stays on this device. Enabled sync checks for changes every 30 seconds.",
        ));
    if !sy.error.is_empty() {
        panel = panel.child(error(p, "sync-error", sy.error.clone()));
    }
    let Some(settings) = &sy.settings else {
        return panel.child(muted(p, "sync-loading", "Loading sync settings…"));
    };
    let is_dirty = dirty(s);
    panel = panel
        .child(switch_row(p, "sync-enabled", "Enable sync", "", sy.enabled, sy.busy, |s: &mut Ui| s.sync.enabled = !s.sync.enabled))
        .child(switch_row(
            p,
            "sync-mock",
            "Use local mock server for testing",
            if sy.mock { "Test data stays in this app's isolated mock database tables." } else { "Enter your server's HTTPS receiving endpoint and API key." },
            sy.mock,
            sy.busy,
            |s: &mut Ui| s.sync.mock = !s.sync.mock,
        ))
        .child(
            col().key("endpoint-field").w_full().gap(space::S1).child(text("Receiving endpoint").key("label").font(type_::LABEL).color(p.t.text)).child(
                TextInput::new(&sy.endpoint)
                    .placeholder("https://server.example/sync")
                    .disabled(sy.mock || sy.busy)
                    .on_edit(|s: &mut Ui| &mut s.sync.endpoint)
                    .build(&p.k)
                    .key("sync-endpoint")
                    .w_full(),
            ),
        )
        .child(
            col().key("key-field").w_full().gap(space::S1).child(text("API key").key("label").font(type_::LABEL).color(p.t.text)).child(
                TextInput::new(&sy.key)
                    .secure()
                    .placeholder(if settings.api_key_configured {
                        "Key saved; leave blank to keep it"
                    } else if sy.mock {
                        "Enter a test key"
                    } else {
                        "Required"
                    })
                    .disabled(sy.busy)
                    .on_edit(|s: &mut Ui| &mut s.sync.key)
                    .build(&p.k)
                    .key("sync-key")
                    .w_full(),
            ),
        )
        .child(
            row()
                .key("sync-actions")
                .gap(space::S2)
                .flex_wrap()
                .child(button(p, "save-sync", "Save sync settings", ButtonVariant::Submit, sy.busy, save_sync))
                .child(button(
                    p,
                    "run-sync",
                    if sy.mock { "Test mock sync" } else { "Sync now" },
                    ButtonVariant::Outline,
                    sy.busy || !settings.enabled || is_dirty || settings.running,
                    |s: &mut Ui| act(s, || core::run_sync().map(|_| None), "Sync completed."),
                )),
        );
    if settings.mock && settings.enabled && !is_dirty {
        let mut scenarios = row().key("scenario-buttons").gap(space::S2).flex_wrap();
        for (scenario, label) in [
            ("remote_edit", "Simulate another device"),
            ("conflict", "Simulate conflicting edits"),
            ("failure", "Simulate server failure"),
            ("authentication", "Simulate rejected key"),
        ] {
            scenarios = scenarios.child(button(p, scenario, label, ButtonVariant::Neutral, sy.busy || settings.running, move |s: &mut Ui| {
                act(s, move || core::mock_scenario(scenario).map(Some), "Mock scenario prepared. Click Test mock sync.")
            }));
        }
        panel = panel.child(
            col()
                .key("sync-mock-panel")
                .w_full()
                .gap(space::S2)
                .child(muted(p, "mock-hint", "Mock scenarios: prepare a test, then click Test mock sync. Failures happen once so the next attempt can recover."))
                .child(scenarios),
        );
    }
    let mut status = col().key("sync-status").w_full().gap(space::S1);
    if is_dirty {
        status = status.child(muted(p, "dirty", "Save settings before syncing."));
    }
    let state = if sy.busy || settings.running {
        "Working…".to_owned()
    } else if !sy.message.is_empty() {
        sy.message.clone()
    } else if settings.enabled {
        "Sync enabled".into()
    } else {
        "Sync disabled".into()
    };
    let last = settings.last_success.as_deref().filter(|v| !v.is_empty()).map(timefmt::display).unwrap_or_else(|| "Never".into());
    status = status
        .child(text(state).key("state").font(type_::BODY).color(p.t.text).role("status"))
        .child(muted(p, "last", format!("Last successful sync: {last}")));
    if let Some(e) = &settings.error {
        status = status.child(error(p, "settings-error", e.clone()));
    }
    status = status.child(text(format!("Device: {}", settings.device_id)).key("device").font(type_::MONO_CAPTION).color(p.t.text_secondary));
    panel = panel.child(status);
    for c in &sy.conflicts {
        let local_name = c.local["filename"].as_str().unwrap_or("").to_owned();
        let (id_local, id_server) = (c.recording_id.clone(), c.recording_id.clone());
        panel = panel.child(
            col()
                .key(format!("conflict-{}", c.recording_id))
                .w_full()
                .gap(space::S2)
                .padding(space::S3)
                .radius(8.0)
                .border(1.0, p.danger)
                .child(text(format!("Conflicting edits: {local_name}")).key("title").font(type_::TITLE).color(p.t.text))
                .child(muted(p, "hint", "Choose the version to keep. Keeping this device's version uploads it on the next sync."))
                .child(
                    row()
                        .key("versions")
                        .w_full()
                        .gap(space::S2)
                        .child(version(p, "local", "This device", &c.local))
                        .child(version(p, "server", "Server", &c.server)),
                )
                .child(
                    row()
                        .key("choices")
                        .gap(space::S2)
                        .child(button(p, "keep-local", "Keep this device", ButtonVariant::Outline, sy.busy, move |s: &mut Ui| {
                            let id = id_local.clone();
                            act(s, move || core::resolve_sync_conflict(&id, "local").map(|_| None), "Local version kept. Sync to upload it.")
                        }))
                        .child(button(p, "keep-server", "Keep server", ButtonVariant::Neutral, sy.busy, move |s: &mut Ui| {
                            let id = id_server.clone();
                            act(s, move || core::resolve_sync_conflict(&id, "server").map(|_| None), "Server version applied.")
                        })),
                ),
        );
    }
    panel
}

fn appearance_card(s: &Ui, p: &P) -> El {
    let labels = Appearance::ALL.map(Appearance::label);
    let selected = Appearance::ALL.iter().position(|a| *a == s.appearance);
    let picker = SegmentedControl::new(&labels, selected)
        .on_pick(|s: &mut Ui, index, _| {
            let Some(&appearance) = Appearance::ALL.get(index) else { return };
            match core::set_appearance(appearance) {
                Ok(()) => s.appearance = appearance,
                Err(e) => s.settings.error = e.detail,
            }
        })
        .build(&p.k)
        .key("appearance-picker");
    card(p, "appearance-card").child(
        row()
            .key("appearance-row")
            .w_full()
            .gap(space::S6)
            .center_y()
            .justify_between()
            .child(
                col()
                    .key("text")
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(space::S1)
                    .child(text("Appearance").key("label").font(type_::TITLE).color(p.t.text))
                    .child(muted(p, "description", "Auto follows your Windows light or dark app mode.")),
            )
            .child(picker),
    )
}

pub fn view(s: &Ui, p: &P) -> El {
    let st = &s.settings;
    let mut page = col()
        .key("settings-page")
        .w_full()
        .gap(space::S6)
        .child(h1(p, "title", "Settings"))
        .child(appearance_card(s, p))
        .child(muted(p, "intro", "Choose what happens after your recordings are transcribed. Changes apply to future processing jobs."));
    if !st.error.is_empty() {
        page = page.child(error(p, "error", st.error.clone()));
    }
    match &st.settings {
        None if st.error.is_empty() => page = page.child(muted(p, "loading", "Loading settings…")),
        None => {}
        Some(settings) => {
            let mut auto_title = col()
                .key("auto-title-col")
                .w_full()
                .gap(space::S2)
                .child(switch_row(
                    p,
                    "auto-title",
                    "Auto title",
                    "Generate and apply a short title from the transcript using OpenAI GPT-5.6 Luna through OpenRouter. You can edit it with Rename. Enabling this sends transcript text to OpenRouter.",
                    settings.auto_title,
                    st.saving,
                    |s: &mut Ui| toggle(s, false),
                ));
            if !settings.openrouter_configured {
                auto_title = auto_title.child(muted(p, "openrouter-hint", "Add your OpenRouter API key under API keys below to generate titles."));
            }
            page = page.child(
                card(p, "settings-card")
                    .gap(space::S6)
                    .child(switch_row(
                        p,
                        "auto-delete",
                        "Auto-delete audio",
                        "Delete audio permanently after the transcript is safely saved. Keep the transcript in Recordings and Search. If transcription fails, audio is kept so you can retry.",
                        settings.auto_delete_audio,
                        st.saving,
                        |s: &mut Ui| toggle(s, true),
                    ))
                    .child(auto_title),
            );
        }
    }
    page = page.child(super::keys::view(s, p)).child(sync_panel(s, p));
    let status = if st.saving { "Saving…" } else if st.saved { "Settings saved." } else { "" };
    page.child(text(status).key("save-status").font(type_::BODY).color(p.t.text_secondary).role("status"))
}
