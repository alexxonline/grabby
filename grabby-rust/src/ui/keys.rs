//! The API keys section of Settings: keys entered here are, encrypted with Windows DPAPI into `configs.dat`, never shown again.

use rune::tok::{space, type_};
use rune::{El, TextEdit, col, row, text};
use rune_kit::{ButtonVariant, TextInput};

use super::{P, Ui, button, card, error, h2, muted};
use crate::{config, secrets};

struct KeyInfo {
    name: &'static str,
    title: &'static str,
    description: &'static str,
}

const KEYS: [KeyInfo; 3] = [
    KeyInfo {
        name: secrets::ELEVENLABS,
        title: "ElevenLabs",
        description: "Required. Used for live captions and for transcribing recordings with Scribe v3. Create one at elevenlabs.io > Settings > API keys.",
    },
    KeyInfo {
        name: secrets::OPENROUTER,
        title: "OpenRouter",
        description: "Optional. Used by Auto title to name recordings from their transcript.",
    },
    KeyInfo {
        name: secrets::SYNC,
        title: "Text sync",
        description: "Optional. Sent as the Bearer token to your sync endpoint. You can also set it in Text sync below.",
    },
];

#[derive(Default)]
pub struct KeysPage {
    pub edits: [TextEdit; 3],
    pub stored: [bool; 3],
    /// Per key: a confirmation or an error.
    pub messages: [Option<(bool, String)>; 3],
}

pub fn enter(s: &mut Ui) {
    for (i, key) in KEYS.iter().enumerate() {
        s.keys.stored[i] = secrets::is_set(key.name);
    }
    s.keys.messages = Default::default();
}

/// Moves the typed key out of the field and wipes the field's buffer.
fn take_input(edit: &mut TextEdit) -> String {
    let value = edit.text.trim().to_owned();
    // SAFETY: zeroing a String's bytes keeps it valid UTF-8.
    secrets::wipe(unsafe { edit.text.as_bytes_mut() });
    edit.replace_text(String::new());
    value
}

fn save(s: &mut Ui, i: usize) {
    let mut value = take_input(&mut s.keys.edits[i]);
    let result = secrets::set(KEYS[i].name, &value);
    // SAFETY: as above.
    secrets::wipe(unsafe { value.as_bytes_mut() });
    s.keys.messages[i] = Some(match result {
        Ok(()) => (true, "Saved and encrypted for your Windows account.".into()),
        Err(e) => (false, e),
    });
    after_change(s, i);
}

fn remove(s: &mut Ui, i: usize) {
    s.keys.messages[i] = Some(match secrets::remove(KEYS[i].name) {
        Ok(()) => (true, "Removed.".into()),
        Err(e) => (false, e),
    });
    after_change(s, i);
}

fn key_card(s: &Ui, p: &P, i: usize) -> El {
    let info = &KEYS[i];
    let stored = s.keys.stored[i];
    let env_only = !stored && config::from_process_env(info.name);
    let status = if stored {
        "Saved (encrypted)"
    } else if env_only {
        "Using the environment variable"
    } else {
        "Not set"
    };
    let mut c = card(p, &format!("key-{}", info.name))
        .child(
            row()
                .key("header")
                .w_full()
                .center_y()
                .justify_between()
                .child(h2(p, "title", info.title))
                .child(text(status).key("status").font(type_::LABEL).color(if stored || env_only { p.accent } else { p.t.text_secondary }).role("status")),
        )
        .child(muted(p, "description", info.description))
        .child(
            row()
                .key("entry")
                .w_full()
                .gap(space::S2)
                .center_y()
                .child(
                    TextInput::new(&s.keys.edits[i])
                        .secure()
                        .label(format!("{} API key", info.title))
                        .placeholder(if stored { "Enter a new key to replace the saved one" } else { "Paste your API key" })
                        .on_edit(move |s: &mut Ui| &mut s.keys.edits[i])
                        .on_submit(move |s: &mut Ui, _| save(s, i))
                        .build(&p.k)
                        .key("input")
                        .grow(1.0),
                )
                .child(button(p, "save", "Save", ButtonVariant::Submit, s.keys.edits[i].text.trim().is_empty(), move |s: &mut Ui| save(s, i)))
                .child(button(p, "remove", "Remove", ButtonVariant::Ghost, !stored, move |s: &mut Ui| remove(s, i))),
        );
    match &s.keys.messages[i] {
        Some((true, m)) => c = c.child(muted(p, "message", m.clone()).role("status")),
        Some((false, m)) => c = c.child(error(p, "message", m.clone())),
        None => {}
    }
    c
}

/// The API keys section of the Settings page.
pub fn view(s: &Ui, p: &P) -> El {
    let mut page = col()
        .key("api-keys")
        .w_full()
        .gap(space::S3)
        .child(h2(p, "title", "API keys"))
        .child(muted(
            p,
            "intro",
            format!(
                "Keys are encrypted with Windows DPAPI for your Windows account and stored in {}. Only you, signed in on this PC, can decrypt them. Saved keys are never shown again; enter a new one to replace it.",
                secrets::store_path().display()
            ),
        ));
    for i in 0..KEYS.len() {
        page = page.child(key_card(s, p, i));
    }
    page
}

/// Keeps the rest of Settings in step: the auto-title hint and the sync key status.
fn after_change(s: &mut Ui, i: usize) {
    s.keys.stored[i] = secrets::is_set(KEYS[i].name);
    if let Ok(settings) = crate::core::get_settings() {
        s.settings.settings = Some(settings);
    }
    if let Ok(sync) = crate::core::sync_settings() {
        s.sync.settings = Some(sync);
    }
}

/// Re-reads which keys are stored (the sync panel can save the sync key too).
pub fn refresh_status(s: &mut Ui) {
    for (i, key) in KEYS.iter().enumerate() {
        s.keys.stored[i] = secrets::is_set(key.name);
    }
}
