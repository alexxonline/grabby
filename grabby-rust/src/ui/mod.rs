mod keys;
mod notes;
mod player;
mod recorder;
mod recordings;
mod search;
mod settings;
mod transcription;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rune::tok::{self, Theme, radius, space, type_};
use rune::{App, AppHandle, AppSpec, Color, Cx, El, Key, KeyEvent, ScrollSpec, TextStyle, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Toast, ToastRegion};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Recorder,
    Recordings,
    Search,
    Settings,
    Transcription,
}

pub struct Ui {
    pub page: Page,
    pub rec: recorder::RecorderPage,
    pub recs: recordings::RecordingsPage,
    pub search: search::SearchPage,
    pub settings: settings::SettingsPage,
    pub keys: keys::KeysPage,
    pub sync: settings::SyncPanel,
    pub tr: transcription::TranscriptionPage,
    pub notes: notes::NotesState,
    pub detail_tab: usize,
    pub player: Option<player::Player>,
    pub appearance: crate::core::Appearance,
    pub toast: Option<(String, Instant)>,
}

const TOAST_SECONDS: u64 = 4;

pub fn notify(s: &mut Ui, message: impl Into<String>) {
    s.toast = Some((message.into(), Instant::now()));
}

fn init() -> Ui {
    let mut ui = Ui {
        page: Page::Recorder,
        rec: Default::default(),
        recs: Default::default(),
        search: Default::default(),
        settings: Default::default(),
        keys: Default::default(),
        sync: Default::default(),
        tr: Default::default(),
        notes: Default::default(),
        detail_tab: 0,
        player: None,
        appearance: crate::core::get_appearance(),
        toast: None,
    };
    recorder::enter(&mut ui);
    ui
}

static HANDLE: OnceLock<AppHandle<Ui>> = OnceLock::new();

pub fn spawn_task<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(&mut Ui, T) + Send + 'static) {
    let Some(handle) = HANDLE.get().cloned() else { return };
    std::thread::spawn(move || {
        let result = work();
        crate::db::close_connection();
        handle.update(move |s, _| done(s, result));
    });
}

pub fn navigate(s: &mut Ui, page: Page) {
    notes::flush(s);
    s.page = page;
    match page {
        Page::Recorder => recorder::enter(s),
        Page::Recordings => recordings::enter(s),
        Page::Search => {}
        Page::Settings => settings::enter(s),
        Page::Transcription => {}
    }
}

pub fn open_transcription(s: &mut Ui, filename: &str, notes_tab: bool) {
    navigate(s, Page::Transcription);
    transcription::enter(s, filename, notes_tab);
}

fn poll(s: &mut Ui, _clock: f64, tx: &mut Tx) -> bool {
    tx.wake_after(Duration::from_millis(250));
    let mut changed = notes::tick(s);
    changed |= match s.page {
        Page::Recorder => recorder::tick(s),
        Page::Recordings => recordings::tick(s),
        Page::Search => false,
        Page::Settings => settings::tick(s),
        Page::Transcription => transcription::tick(s),
    };
    changed |= player::tick(s);
    if s.toast.as_ref().is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(TOAST_SECONDS)) {
        s.toast = None;
        changed = true;
    }
    changed
}

fn on_key(s: &mut Ui, ev: &KeyEvent, _: &mut Tx) -> bool {
    if ev.key == Key::Escape {
        return recordings::cancel_confirm(s);
    }
    if !ev.mods.cmd || ev.mods.alt {
        return false;
    }
    if ev.is_command('r') {
        recorder::toggle(s);
        return true;
    }
    for (c, page) in [('1', Page::Recorder), ('2', Page::Recordings), ('3', Page::Search), ('4', Page::Settings)] {
        if ev.is_command(c) {
            navigate(s, page);
            return true;
        }
    }
    if ev.is_command('f') {
        navigate(s, Page::Search);
        return true;
    }
    false
}

fn on_close(s: &mut Ui, _: &mut Tx) {
    notes::flush(s);
}

pub struct P {
    pub t: Theme,
    pub k: Kit,
    pub accent: Color,
    pub danger: Color,
    pub mark: Color,
}

impl P {
    fn new(dark: bool) -> Self {
        P {
            t: tok::theme(dark),
            k: Kit::neutral(dark),
            accent: tok::accent(0, dark),
            danger: if dark { rune::oklch(0.72, 0.17, 25.0) } else { rune::oklch(0.55, 0.2, 27.0) },
            mark: if dark { rune::oklcha(0.85, 0.16, 95.0, 0.35) } else { rune::oklcha(0.9, 0.17, 95.0, 0.8) },
        }
    }
}

pub const H1: TextStyle = TextStyle::new(22.0, 650.0);
pub const H2: TextStyle = TextStyle::new(16.0, 600.0);

pub fn h1(p: &P, key: &str, s: impl Into<String>) -> El {
    text(s.into()).key(key).font(H1).color(p.t.text).role("heading").level(1)
}

pub fn h2(p: &P, key: &str, s: impl Into<String>) -> El {
    text(s.into()).key(key).font(H2).color(p.t.text).role("heading").level(2)
}

pub fn muted(p: &P, key: &str, s: impl Into<String>) -> El {
    text(s.into()).key(key).font(type_::BODY).color(p.t.text_secondary).wrap()
}

pub fn error(p: &P, key: &str, s: impl Into<String>) -> El {
    text(s.into()).key(key).font(type_::BODY).color(p.danger).wrap().role("alert")
}

pub fn card(p: &P, key: &str) -> El {
    col().key(key).w_full().padding(space::S6).gap(space::S3).radius(radius::LG).fill(p.t.card).border(1.0, p.t.card_border)
}

pub fn button(p: &P, key: &str, label: impl Into<String>, variant: ButtonVariant, disabled: bool, f: impl Fn(&mut Ui) + 'static) -> El {
    Button::new(label).variant(variant).control_size(ControlSize::Small).disabled(disabled).on_press(f).build(&p.k).key(key)
}

pub fn button_tx(p: &P, key: &str, label: impl Into<String>, variant: ButtonVariant, disabled: bool, f: impl Fn(&mut Ui, &mut Tx) + 'static) -> El {
    Button::new(label).variant(variant).control_size(ControlSize::Small).disabled(disabled).on_press_tx(f).build(&p.k).key(key)
}

pub fn link(p: &P, key: &str, label: impl Into<String>, f: impl Fn(&mut Ui) + 'static) -> El {
    let label = label.into();
    text(label.clone()).key(key).font(type_::BODY).color(p.accent).role("link").label(label).focusable(true).on_press(f)
}

pub fn recording_tabs(s: &Ui, p: &P, live: bool, transcript: El) -> El {
    let labels = [if live { "Live transcript" } else { "Transcript" }, "Notes"];
    let tabs = SegmentedControl::new(&labels, Some(s.detail_tab))
        .on_pick(|s: &mut Ui, index, _| {
            if s.detail_tab == 1 && index != 1 {
                notes::flush(s);
            }
            s.detail_tab = index;
        })
        .build(&p.k)
        .key("detail-tabs");
    let panel = if s.detail_tab == 0 { transcript.key("detail-panel-0") } else { notes::view(s, p).key("detail-panel-1") };
    col().key("recording-details").w_full().gap(space::S3).child(row().key("tabs-row").child(tabs)).child(panel)
}

fn nav(s: &Ui, p: &P) -> El {
    let mut links = row().key("nav-links").gap(space::S1);
    for (page, label) in [(Page::Recorder, "Recorder"), (Page::Recordings, "Recordings"), (Page::Search, "Search"), (Page::Settings, "Settings")] {
        let active = s.page == page || (page == Page::Recordings && s.page == Page::Transcription);
        links = links.child(
            Button::new(label)
                .variant(if active { ButtonVariant::Neutral } else { ButtonVariant::Ghost })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut Ui| navigate(s, page))
                .build(&p.k)
                .key(format!("nav-{label}")),
        );
    }
    row()
        .key("navbar")
        .role("navigation")
        .w_full()
        .h(52.0)
        .px(space::S6)
        .center_y()
        .justify_between()
        .fill(p.t.card)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.t.separator)
        .child(text("Grabby").key("nav-brand").font(H2).color(p.accent))
        .child(links)
}

fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = match s.appearance {
        crate::core::Appearance::Auto => cx.system_dark(),
        crate::core::Appearance::Light => false,
        crate::core::Appearance::Dark => true,
    };
    let p = P::new(dark);
    let page = match s.page {
        Page::Recorder => recorder::view(s, &p),
        Page::Recordings => recordings::view(s, &p),
        Page::Search => search::view(s, &p),
        Page::Settings => settings::view(s, &p),
        Page::Transcription => transcription::view(s, &p),
    };
    let content = col()
        .key(format!("page-{:?}", s.page))
        .w_full()
        .max_w(900.0)
        .self_center()
        .padding(space::S6)
        .gap(space::S6)
        .child(page);
    let mut window = col()
        .key("window")
        .w_full()
        .h_full()
        .fill(p.t.bg)
        .child(nav(s, &p))
        .child(
            col()
                .key("main")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(content),
        );
    if let Some(pl) = &s.player {
        window = window.child(player::view(pl, &p));
    }
    if let Some((message, _)) = &s.toast {
        let toast = Toast::new("toast", message.clone()).on_close(|s: &mut Ui| s.toast = None).build(&p.k);
        window = window.child(ToastRegion::new().toast(toast).build(&p.k));
    }
    window
}

pub fn run() {
    #[cfg(windows)]
    rune::set_app_icon_resource(1);
    let app = App::new(AppSpec::<Ui> {
        poll: Some(poll),
        scenes: &[
            ("recorder", |_| {}),
            ("recordings", |s| navigate(s, Page::Recordings)),
            ("search", |s| navigate(s, Page::Search)),
            ("settings", |s| navigate(s, Page::Settings)),
            ("settings-light", |s| {
                navigate(s, Page::Settings);
                s.appearance = crate::core::Appearance::Light;
            }),
            ("recorder-light", |s| s.appearance = crate::core::Appearance::Light),
            ("toast", |s| notify(s, "Recording discarded")),
        ],
        on_key: Some(on_key),
        min_size: Some((640.0, 480.0)),
        ..AppSpec::new("Grabby", (1040.0, 760.0), init, view)
    })
    .on_close(on_close);
    let _ = HANDLE.set(app.handle());
    app.run();
}
