//! Records system audio (WASAPI loopback) and microphone, then mixes them
//! (port of `app/recorder.py`).
//!
//! Capture goes to raw PCM files in a hidden `.capture-<uuid>` folder with a JSON manifest, so
//! an interrupted capture can be recovered and saved later ("Retry Save").

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};

use crate::audio::{Resampler, add, bytes_to_i16, convert_channels, i16_to_bytes};
use crate::realtime::{LiveSnapshot, LiveTranscript};
use crate::{config, db};

pub static RECORDER: LazyLock<MeetingRecorder> = LazyLock::new(MeetingRecorder::default);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Params {
    lb_rate: u32,
    lb_channels: u16,
    mic_rate: u32,
    mic_channels: u16,
}

impl Default for Params {
    fn default() -> Self {
        Params { lb_rate: 48000, lb_channels: 2, mic_rate: 44100, mic_channels: 1 }
    }
}

#[derive(Default)]
struct State {
    thread: Option<JoinHandle<()>>,
    capture_dir: Option<PathBuf>,
    current_file: Option<String>,
    started_at: Option<DateTime<Local>>,
    error: Option<String>,
    live: Option<Arc<LiveTranscript>>,
    saving: bool,
    save_pending: bool,
    saved_output: Option<PathBuf>,
    notes_id: Option<String>,
    params: Params,
}

#[derive(Default)]
pub struct MeetingRecorder {
    state: Mutex<State>,
    recording: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct RecorderStatus {
    pub recording: bool,
    pub save_pending: bool,
    pub saving: bool,
    pub error: Option<String>,
    pub filename: Option<String>,
    pub started_at: Option<String>,
    pub live: Option<LiveSnapshot>,
    pub notes_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    name: String,
    notes_id: Option<String>,
    lb_rate: i64,
    lb_channels: i64,
    mic_rate: i64,
    mic_channels: i64,
    saved_output: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    /// Invalid request (maps to status 400).
    #[error("{0}")]
    Invalid(String),
    /// Conflicting state, e.g. already recording (maps to status 409).
    #[error("{0}")]
    Conflict(String),
}

fn thread_alive(t: &Option<JoinHandle<()>>) -> bool {
    t.as_ref().is_some_and(|h| !h.is_finished())
}

fn has_data(path: &Path, channels: u16) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() >= u64::from(channels) * 2)
}

impl MeetingRecorder {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }

    pub fn current_file(&self) -> Option<String> {
        self.lock().current_file.clone()
    }

    pub fn save_pending(&self) -> bool {
        self.lock().save_pending
    }

    pub fn notes_id(&self) -> Option<String> {
        self.lock().notes_id.clone()
    }

    pub fn status(&self) -> RecorderStatus {
        let s = self.lock();
        let recording = self.is_recording();
        RecorderStatus {
            recording,
            save_pending: s.save_pending,
            saving: s.saving,
            error: s.error.clone(),
            filename: s.current_file.clone(),
            started_at: s.started_at.map(crate::timefmt::local_aware),
            live: if recording { s.live.as_ref().map(|l| l.snapshot()) } else { None },
            notes_id: s.notes_id.clone(),
        }
    }

    fn capture_failed(&self, s: &mut State, message: String) {
        s.error = Some(format!("Audio capture failed: {message}"));
        self.recording.store(false, Ordering::SeqCst);
        if let Some(dir) = &s.capture_dir {
            let p = s.params;
            s.save_pending = has_data(&dir.join("loopback.pcm"), p.lb_channels) || has_data(&dir.join("mic.pcm"), p.mic_channels);
        }
        if let Some(live) = &s.live {
            live.stop();
        }
    }

    fn write_manifest(s: &State) -> std::io::Result<()> {
        let Some(dir) = &s.capture_dir else { return Ok(()) };
        let manifest = Manifest {
            name: s.current_file.clone().unwrap_or_default(),
            notes_id: s.notes_id.clone(),
            lb_rate: i64::from(s.params.lb_rate),
            lb_channels: i64::from(s.params.lb_channels),
            mic_rate: i64::from(s.params.mic_rate),
            mic_channels: i64::from(s.params.mic_channels),
            saved_output: s.saved_output.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()),
        };
        let temporary = dir.join("capture.tmp");
        std::fs::write(&temporary, serde_json::to_vec(&manifest)?)?;
        std::fs::rename(temporary, dir.join("capture.json"))
    }

    /// Restore one interrupted capture at a time, without guessing raw PCM formats.
    pub fn recover_pending(&self) {
        let mut s = self.lock();
        if self.is_recording() || s.saving || s.save_pending || thread_alive(&s.thread) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(config::recordings_dir()) else { return };
        let mut manifests: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".capture-"))
            .map(|e| e.path().join("capture.json"))
            .filter(|p| p.is_file())
            .collect();
        manifests.sort();
        for manifest in manifests {
            let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
            let Ok(data) = serde_json::from_str::<Manifest>(&text) else { continue };
            let Ok(name) = validate_stem(&data.name) else { continue };
            let params = [data.lb_rate, data.lb_channels, data.mic_rate, data.mic_channels];
            if params.iter().any(|&v| v <= 0 || v > i64::from(u32::MAX)) {
                continue;
            }
            if let Some(saved) = &data.saved_output {
                let plain = Path::new(saved).file_name().and_then(|n| n.to_str()) == Some(saved.as_str());
                if !plain || (*saved != format!("{name}.wav") && *saved != format!("{name}.mp3")) {
                    continue;
                }
            }
            let dir = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
            let has_pcm = std::fs::read_dir(&dir).is_ok_and(|rd| {
                rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "pcm") && e.metadata().is_ok_and(|m| m.len() > 0))
            });
            if !has_pcm {
                continue;
            }
            s.capture_dir = Some(dir);
            s.current_file = Some(name);
            s.notes_id = data.notes_id;
            s.params = Params {
                lb_rate: data.lb_rate as u32,
                lb_channels: data.lb_channels as u16,
                mic_rate: data.mic_rate as u32,
                mic_channels: data.mic_channels as u16,
            };
            s.saved_output = data.saved_output.map(|n| config::recordings_dir().join(n));
            s.save_pending = true;
            s.error = Some("Interrupted audio capture recovered. Use Retry Save.".into());
            return;
        }
    }

    pub fn start(&'static self, filename: Option<&str>) -> Result<(), RecorderError> {
        self.recover_pending();
        let mut s = self.lock();
        if s.save_pending {
            return Err(RecorderError::Conflict("Save the previous recording before starting another. Use Retry Save.".into()));
        }
        if self.is_recording() || s.saving || thread_alive(&s.thread) {
            return Err(RecorderError::Conflict("Already recording".into()));
        }
        let started_at = Local::now();
        let default_name = format!("recording_{}", started_at.format("%Y%m%d_%H%M%S"));
        let filename = validate_stem(filename.unwrap_or(&default_name)).map_err(RecorderError::Invalid)?;
        let dir = config::recordings_dir();
        std::fs::create_dir_all(&dir).map_err(|e| RecorderError::Conflict(e.to_string()))?;
        let filename = unique_stem(&filename).map_err(|e| RecorderError::Conflict(e.to_string()))?;
        let notes = db::create_notes(None).map_err(|e| RecorderError::Conflict(e.to_string()))?;
        let capture_dir = dir.join(format!(".capture-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&capture_dir).map_err(|e| RecorderError::Conflict(e.to_string()))?;
        s.started_at = Some(started_at);
        s.current_file = Some(filename);
        s.notes_id = Some(notes.id);
        s.capture_dir = Some(capture_dir);
        s.error = None;
        s.saved_output = None;
        s.params = Params::default();
        self.recording.store(true, Ordering::SeqCst);
        let live = Arc::new(LiveTranscript::new());
        s.live = Some(live.clone());
        let started = live.start().and_then(|()| {
            std::thread::Builder::new().name("audio-capture".into()).spawn(move || self.record_loop())
        });
        match started {
            Ok(handle) => {
                s.thread = Some(handle);
                Ok(())
            }
            Err(e) => {
                self.capture_failed(&mut s, e.to_string());
                Err(RecorderError::Conflict(s.error.clone().unwrap_or_default()))
            }
        }
    }

    pub fn stop(&self) -> Result<PathBuf, RecorderError> {
        let (thread, live) = {
            let mut s = self.lock();
            if s.saving {
                return Err(RecorderError::Conflict("Recording is already being saved".into()));
            }
            if !self.is_recording() && !s.save_pending {
                return Err(RecorderError::Conflict(s.error.clone().unwrap_or_else(|| "Not recording".into())));
            }
            self.recording.store(false, Ordering::SeqCst);
            s.save_pending = true;
            s.saving = true;
            (s.thread.take(), s.live.clone())
        };
        let result = (|| -> Result<PathBuf, String> {
            if let Some(live) = live {
                live.stop();
            }
            if let Some(thread) = thread {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !thread.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !thread.is_finished() {
                    self.lock().thread = Some(thread);
                    return Err("Audio capture is still stopping; please wait".into());
                }
                let _ = thread.join();
            }
            self.mix_and_save()
        })();
        let mut s = self.lock();
        s.saving = false;
        match result {
            Ok(output) => {
                s.save_pending = false;
                s.error = None;
                Ok(output)
            }
            Err(e) => {
                let message = format!("Could not save recording: {e}. Resolve the problem and use Retry Save.");
                s.error = Some(message.clone());
                Err(RecorderError::Conflict(message))
            }
        }
    }

    fn record_loop(&'static self) {
        if let Err(e) = self.capture() {
            let mut s = self.lock();
            self.capture_failed(&mut s, e);
        }
    }

    /// Raw PCM avoids both unbounded RAM and WAV's 4 GiB capture limit.
    fn capture(&'static self) -> Result<(), String> {
        let host = cpal::default_host();
        // WASAPI loopback: an input stream on the default output (render) device.
        let loopback = host.default_output_device().ok_or("No WASAPI loopback device found")?;
        let mic = host.default_input_device().ok_or("No default input device available")?;
        let lb_config = loopback.default_output_config().map_err(|e| e.to_string())?;
        let mic_config = mic.default_input_config().map_err(|e| e.to_string())?;
        let params = Params {
            lb_rate: lb_config.sample_rate(),
            lb_channels: lb_config.channels(),
            mic_rate: mic_config.sample_rate(),
            mic_channels: mic_config.channels().max(1),
        };
        let (dir, live) = {
            let mut s = self.lock();
            s.params = params;
            Self::write_manifest(&s).map_err(|e| e.to_string())?;
            (s.capture_dir.clone().ok_or("No capture folder")?, s.live.clone())
        };
        let mut outputs = [
            BufWriter::new(File::create(dir.join("loopback.pcm")).map_err(|e| e.to_string())?),
            BufWriter::new(File::create(dir.join("mic.pcm")).map_err(|e| e.to_string())?),
        ];
        let (tx, rx) = mpsc::channel::<Msg>();
        let streams = [
            open_stream(&loopback, &lb_config, 0, tx.clone())?,
            open_stream(&mic, &mic_config, 1, tx)?,
        ];
        for stream in &streams {
            stream.play().map_err(|e| e.to_string())?;
        }
        let sources = [("others", params.lb_rate, params.lb_channels), ("you", params.mic_rate, params.mic_channels)];
        let started = Instant::now();
        let mut loopback_frames: u64 = 0;
        let mut failure = None;
        while self.is_recording() {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Msg::Data(index, data)) => {
                    outputs[index].write_all(&data).map_err(|e| e.to_string())?;
                    if index == 0 {
                        loopback_frames += (data.len() / (2 * params.lb_channels as usize)) as u64;
                    }
                    if let Some(live) = &live {
                        let (source, rate, channels) = sources[index];
                        live.feed(source, &data, rate, channels);
                    }
                }
                Ok(Msg::Error(e)) => {
                    failure = Some(e);
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            // WASAPI delivers no loopback packets while nothing plays; keep the system track
            // aligned with the microphone by writing that silence.
            let expected = (started.elapsed().as_secs_f64() * f64::from(params.lb_rate)) as u64;
            let slack = u64::from(params.lb_rate) / 4;
            if loopback_frames + slack < expected {
                let missing = expected - u64::from(params.lb_rate) / 20 - loopback_frames;
                let zeros = vec![0u8; (missing as usize) * 2 * params.lb_channels as usize];
                outputs[0].write_all(&zeros).map_err(|e| e.to_string())?;
                loopback_frames += missing;
            }
        }
        drop(streams);
        while let Ok(Msg::Data(index, data)) = rx.try_recv() {
            outputs[index].write_all(&data).map_err(|e| e.to_string())?;
        }
        for output in &mut outputs {
            output.flush().map_err(|e| e.to_string())?;
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn mix_and_save(&self) -> Result<PathBuf, String> {
        let (capture_dir, current, params, saved, notes_id) = {
            let s = self.lock();
            (s.capture_dir.clone(), s.current_file.clone().unwrap_or_default(), s.params, s.saved_output.clone(), s.notes_id.clone())
        };
        let capture_dir = capture_dir.ok_or("No captured audio to save")?;
        // Keep intermediate and incomplete exports out of the recordings listing.
        let mixed_wav = capture_dir.join("mixed.wav");
        let encoded_mp3 = capture_dir.join("mixed.mp3");
        let output_file = match saved {
            Some(saved) => saved,
            None => {
                mix_to_wav(&capture_dir, params, &mixed_wav).map_err(|e| e.to_string())?;
                let output = match export_mp3(&mixed_wav, &encoded_mp3) {
                    Ok(()) => {
                        let out = config::recordings_dir().join(format!("{current}.mp3"));
                        std::fs::rename(&encoded_mp3, &out).map_err(|e| e.to_string())?;
                        out
                    }
                    Err(e) => {
                        eprintln!("MP3 export unavailable; keeping WAV: {e}");
                        let out = config::recordings_dir().join(format!("{current}.wav"));
                        std::fs::rename(&mixed_wav, &out).map_err(|e| e.to_string())?;
                        out
                    }
                };
                // If attaching notes fails, retry it without exporting a second file.
                let mut s = self.lock();
                s.saved_output = Some(output.clone());
                Self::write_manifest(&s).map_err(|e| e.to_string())?;
                output
            }
        };
        let name = output_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(id) = notes_id {
            db::attach_notes(&id, &name).map_err(|e| e.to_string())?;
        }
        // Retire recovery first: leftover PCM after a cleanup failure must not make a
        // successfully saved meeting appear interrupted again.
        for file in ["capture.json", "capture.tmp", "loopback.pcm", "mic.pcm", "mixed.wav", "mixed.mp3"] {
            let _ = std::fs::remove_file(capture_dir.join(file));
        }
        if std::fs::remove_dir(&capture_dir).is_err() {
            eprintln!("Recording saved, but temporary audio cleanup failed");
        }
        self.lock().capture_dir = None;
        Ok(output_file)
    }
}

enum Msg {
    Data(usize, Vec<u8>),
    Error(String),
}

fn open_stream(device: &cpal::Device, config: &cpal::SupportedStreamConfig, index: usize, tx: mpsc::Sender<Msg>) -> Result<cpal::Stream, String> {
    use cpal::SampleFormat as F;
    let stream_config: cpal::StreamConfig = config.config();
    let err_tx = tx.clone();
    let on_error = move |err: cpal::Error| {
        // Device switches and glitches are reported but don't end the meeting.
        match err.kind() {
            cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied => {
                eprintln!("audio stream: {err}")
            }
            _ => {
                let _ = err_tx.send(Msg::Error(err.to_string()));
            }
        }
    };
    macro_rules! build {
        ($t:ty) => {
            device.build_input_stream::<$t, _, _>(
                stream_config.clone(),
                move |data: &[$t], _: &cpal::InputCallbackInfo| {
                    let samples: Vec<i16> = data.iter().map(|s| cpal::Sample::to_sample::<i16>(*s)).collect();
                    let _ = tx.send(Msg::Data(index, i16_to_bytes(&samples)));
                },
                on_error,
                None,
            )
        };
    }
    let stream = match config.sample_format() {
        F::I8 => build!(i8),
        F::I16 => build!(i16),
        F::I32 => build!(i32),
        F::U8 => build!(u8),
        F::U16 => build!(u16),
        F::F32 => build!(f32),
        F::F64 => build!(f64),
        other => return Err(format!("Unsupported sample format {other}")),
    };
    stream.map_err(|e| e.to_string())
}

/// If a file with this stem already exists in the recordings folder, append _1, _2, ...
fn unique_stem(stem: &str) -> db::DbResult<String> {
    let existing: Vec<String> = std::fs::read_dir(config::recordings_dir())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    let mut candidate = stem.to_owned();
    let mut counter = 1;
    loop {
        let taken = existing.contains(&candidate.to_lowercase())
            || db::get_transcription(&format!("{candidate}.mp3"))?.is_some()
            || db::get_transcription(&format!("{candidate}.wav"))?.is_some();
        if !taken {
            return Ok(candidate);
        }
        candidate = format!("{stem}_{counter}");
        counter += 1;
    }
}

/// Validates a recording name: 1–160 characters without Windows filename restrictions.
pub fn validate_stem(filename: &str) -> Result<String, String> {
    let filename = filename.strip_suffix(".mp3").unwrap_or(filename);
    let filename = filename.strip_suffix(".wav").unwrap_or(filename);
    // Windows device names remain reserved even when followed by an extension.
    let device = filename.split('.').next().unwrap_or("").trim_end_matches(' ').to_uppercase();
    let mut reserved: Vec<String> = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].map(String::from).to_vec();
    for prefix in ["COM", "LPT"] {
        for digit in "123456789¹²³".chars() {
            reserved.push(format!("{prefix}{digit}"));
        }
    }
    let lower = filename.to_lowercase();
    if filename.trim().is_empty()
        || filename.chars().count() > 160
        || filename.ends_with('.')
        || filename.ends_with(' ')
        || filename.chars().any(|c| (c as u32) < 32 || "<>:\"/\\|?*".contains(c))
        || reserved.contains(&device)
        || lower == "_temp_loopback"
        || lower == "_temp_mic"
    {
        return Err("Use a recording name of 1–160 characters without Windows filename restrictions or reserved names.".into());
    }
    Ok(filename.to_owned())
}

/// Reads a PCM file as aligned blocks converted to the target format.
struct PcmChunks {
    file: Option<File>,
    channels: usize,
    target_channels: usize,
    resampler: Option<Resampler>,
}

impl PcmChunks {
    fn open(path: &Path, rate: u32, channels: u16, target_rate: u32, target_channels: u16) -> Self {
        PcmChunks {
            file: File::open(path).ok(),
            channels: channels as usize,
            target_channels: target_channels as usize,
            resampler: (rate != target_rate).then(|| Resampler::new(rate, target_rate, channels as usize)),
        }
    }

    fn next_block(&mut self) -> std::io::Result<Option<Vec<i16>>> {
        let Some(file) = self.file.as_mut() else { return Ok(None) };
        let frame = self.channels * 2;
        let mut buf = vec![0u8; 4096 * frame];
        let mut filled = 0;
        while filled < buf.len() {
            let n = file.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        let usable = filled / frame * frame;
        if usable == 0 {
            self.file = None;
            return Ok(None);
        }
        let mut samples = bytes_to_i16(&buf[..usable]);
        if let Some(r) = &mut self.resampler {
            samples = r.process(&samples);
        }
        Ok(Some(convert_channels(&samples, self.channels, self.target_channels)))
    }
}

/// Minimal streaming PCM16 WAV writer (header sizes patched on finish).
struct WavWriter {
    out: BufWriter<File>,
    data_len: u64,
}

impl WavWriter {
    fn create(path: &Path, channels: u16, rate: u32) -> std::io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        let block_align = channels * 2;
        out.write_all(b"RIFF\0\0\0\0WAVEfmt ")?;
        out.write_all(&16u32.to_le_bytes())?;
        out.write_all(&1u16.to_le_bytes())?;
        out.write_all(&channels.to_le_bytes())?;
        out.write_all(&rate.to_le_bytes())?;
        out.write_all(&(rate * u32::from(block_align)).to_le_bytes())?;
        out.write_all(&block_align.to_le_bytes())?;
        out.write_all(&16u16.to_le_bytes())?;
        out.write_all(b"data\0\0\0\0")?;
        Ok(WavWriter { out, data_len: 0 })
    }

    fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
        self.data_len += samples.len() as u64 * 2;
        self.out.write_all(&i16_to_bytes(samples))
    }

    fn finish(mut self) -> std::io::Result<()> {
        let data = u32::try_from(self.data_len).unwrap_or(u32::MAX);
        self.out.seek(SeekFrom::Start(4))?;
        self.out.write_all(&data.saturating_add(36).to_le_bytes())?;
        self.out.seek(SeekFrom::Start(40))?;
        self.out.write_all(&data.to_le_bytes())?;
        self.out.flush()
    }
}

fn mix_to_wav(capture_dir: &Path, p: Params, output: &Path) -> std::io::Result<()> {
    let mut system = PcmChunks::open(&capture_dir.join("loopback.pcm"), p.lb_rate, p.lb_channels, p.lb_rate, p.lb_channels);
    let mut mic = PcmChunks::open(&capture_dir.join("mic.pcm"), p.mic_rate, p.mic_channels, p.lb_rate, p.lb_channels);
    let mut wav = WavWriter::create(output, p.lb_channels, p.lb_rate)?;
    let (mut a, mut b): (Vec<i16>, Vec<i16>) = (Vec::new(), Vec::new());
    let (mut a_done, mut b_done) = (false, false);
    loop {
        while !a_done && a.len() < 4096 * p.lb_channels as usize {
            match system.next_block()? {
                Some(block) => a.extend(block),
                None => a_done = true,
            }
        }
        while !b_done && b.len() < 4096 * p.lb_channels as usize {
            match mic.next_block()? {
                Some(block) => b.extend(block),
                None => b_done = true,
            }
        }
        if a.is_empty() && b.is_empty() {
            break;
        }
        // Mix the overlapping part; past the end of one track, the other plays alone.
        let n = match (a_done, b_done) {
            (true, true) => a.len().max(b.len()),
            (true, false) => b.len(),
            (false, true) => a.len(),
            (false, false) => a.len().min(b.len()),
        };
        let left: Vec<i16> = a.drain(..n.min(a.len())).collect();
        let right: Vec<i16> = b.drain(..n.min(b.len())).collect();
        wav.write(&add(&left, &right))?;
    }
    wav.finish()
}

/// Encode with ffmpeg without loading the whole meeting into memory.
fn export_mp3(source: &Path, output: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new("ffmpeg");
    command
        .args(["-nostdin", "-y", "-loglevel", "error", "-i"])
        .arg(source)
        .args(["-b:a", "128k"])
        .arg(output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("ffmpeg exited with {status}")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names() {
        assert_eq!(validate_stem("Weekly sync.mp3").unwrap(), "Weekly sync");
        assert!(validate_stem("CON").is_err());
        assert!(validate_stem("con.txt").is_err());
        assert!(validate_stem("COM¹").is_err());
        assert!(validate_stem("a/b").is_err());
        assert!(validate_stem("name.").is_err());
        assert!(validate_stem("   ").is_err());
        assert!(validate_stem(&"x".repeat(161)).is_err());
        assert!(validate_stem("_temp_mic").is_err());
        assert!(validate_stem("CONSOLE notes").is_ok());
    }

    #[test]
    fn mixes_tracks_of_different_formats() {
        let dir = tempfile::tempdir().unwrap();
        let p = Params { lb_rate: 8000, lb_channels: 2, mic_rate: 4000, mic_channels: 1 };
        std::fs::write(dir.path().join("loopback.pcm"), i16_to_bytes(&[100i16; 8000 * 2])).unwrap();
        std::fs::write(dir.path().join("mic.pcm"), i16_to_bytes(&[50i16; 8000])).unwrap();
        let out = dir.path().join("mixed.wav");
        mix_to_wav(dir.path(), p, &out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(data, bytes.len() - 44);
        let samples = bytes_to_i16(&bytes[44..]);
        // The microphone (2 s at 4 kHz) outlasts the 1 s system track once upsampled.
        assert!(samples.len() >= 8000 * 2 * 2 - 8);
        assert_eq!(samples[100], 150);
        assert_eq!(*samples.last().unwrap(), 50);
    }
}
