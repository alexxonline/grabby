use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::Duration;

use rune::tok::{space, type_};
use rune::{Drag, El, Phase, col, el, row, text};
use rune_kit::ButtonVariant;

use super::{P, Ui, button, error};

pub struct Player {
    pub filename: String,
    pub title: String,
    path: PathBuf,
    _sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    pub duration: f64,
    pub drag: Option<f64>,
    pub error: Option<String>,
    shown: (bool, u64),
}

impl Player {
    pub fn open(path: PathBuf, filename: &str, title: &str) -> Result<Player, String> {
        let sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        let player = rodio::Player::connect_new(sink.mixer());
        player.pause();
        let mut p = Player {
            filename: filename.to_owned(),
            title: title.to_owned(),
            path,
            _sink: sink,
            player,
            duration: 0.0,
            drag: None,
            error: None,
            shown: (false, 0),
        };
        p.load()?;
        Ok(p)
    }

    fn load(&mut self) -> Result<(), String> {
        let file = File::open(&self.path).map_err(|e| e.to_string())?;
        let decoder = rodio::Decoder::new(BufReader::new(file)).map_err(|e| e.to_string())?;
        if let Some(d) = rodio::Source::total_duration(&decoder) {
            self.duration = d.as_secs_f64();
        }
        self.player.append(decoder);
        Ok(())
    }

    pub fn playing(&self) -> bool {
        !self.player.is_paused() && !self.player.empty()
    }

    pub fn position(&self) -> f64 {
        if self.player.empty() { self.duration } else { self.player.get_pos().as_secs_f64() }
    }

    pub fn toggle(&mut self) {
        if self.playing() {
            self.player.pause();
            return;
        }
        if self.player.empty() {
            if let Err(e) = self.load() {
                self.error = Some(e);
                return;
            }
        }
        self.player.play();
    }

    pub fn seek(&mut self, seconds: f64) {
        if self.duration <= 0.0 {
            return;
        }
        if self.player.empty() {
            self.player.pause();
            if self.load().is_err() {
                return;
            }
        }
        if let Err(e) = self.player.try_seek(Duration::from_secs_f64(seconds.clamp(0.0, self.duration))) {
            self.error = Some(e.to_string());
        }
    }
}

pub fn toggle_for(s: &mut Ui, filename: &str, title: &str) {
    if s.player.as_ref().is_some_and(|p| p.filename == filename) {
        s.player = None;
        return;
    }
    s.player = None;
    match crate::core::audio_file(filename).map_err(|e| e.detail).and_then(|path| Player::open(path, filename, title)) {
        Ok(p) => s.player = Some(p),
        Err(e) => s.recs.error = Some(format!("Could not play audio: {e}")),
    }
}

pub fn tick(s: &mut Ui) -> bool {
    let Some(p) = &mut s.player else { return false };
    let now = (p.playing(), p.position() as u64);
    if now != p.shown {
        p.shown = now;
        return true;
    }
    false
}

fn fmt(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "0:00".into();
    }
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

pub fn view(pl: &Player, p: &P) -> El {
    let current = pl.drag.unwrap_or_else(|| pl.position());
    let fraction = if pl.duration > 0.0 { (current / pl.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let progress = el()
        .key("ap-progress")
        .role("slider")
        .label("Seek")
        .range_value(current, 0.0, pl.duration.max(0.0))
        .grow(1.0)
        .h(18.0)
        .on_drag(|s: &mut Ui, d: &mut Drag, _| {
            let Some(pl) = &mut s.player else { return };
            let ratio = f64::from((d.x / d.w.max(1.0)).clamp(0.0, 1.0));
            let target = ratio * pl.duration;
            match d.phase {
                Phase::Down | Phase::Move => pl.drag = Some(target),
                Phase::Up => {
                    pl.drag = None;
                    if !d.cancelled() {
                        pl.seek(target);
                    }
                }
            }
        })
        .center_y()
        .child(rune_kit::Progress::new(Some(fraction)).build(&p.k).key("ap-bar").w_full().a11y_hidden());
    let mut panel = col()
        .key("audio-player")
        .w_full()
        .px(space::S6)
        .py(space::S3)
        .gap(space::S2)
        .fill(p.t.card)
        .border_sides([1.0, 0.0, 0.0, 0.0], p.t.separator)
        .child(
            row()
                .key("ap-header")
                .w_full()
                .center_y()
                .justify_between()
                .child(text(pl.title.clone()).key("ap-title").font(type_::TITLE).color(p.t.text).truncate())
                .child(button(p, "ap-close", "Close", ButtonVariant::Ghost, false, |s: &mut Ui| s.player = None)),
        )
        .child(
            row()
                .key("ap-controls")
                .w_full()
                .gap(space::S3)
                .center_y()
                .child(button(p, "ap-play", if pl.playing() { "Pause" } else { "Play" }, ButtonVariant::Neutral, false, |s: &mut Ui| {
                    if let Some(pl) = &mut s.player {
                        pl.toggle();
                    }
                }))
                .child(text(fmt(current)).key("ap-time").font(type_::MONO_CAPTION).color(p.t.text_secondary))
                .child(progress)
                .child(text(fmt(pl.duration)).key("ap-duration").font(type_::MONO_CAPTION).color(p.t.text_secondary)),
        );
    if let Some(e) = &pl.error {
        panel = panel.child(error(p, "ap-error", e.clone()));
    }
    panel
}
