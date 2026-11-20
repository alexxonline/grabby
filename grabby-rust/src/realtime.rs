//! Ephemeral source-labelled captions; never persisted as the final transcript.
//!
//! Speaks the ElevenLabs realtime speech-to-text WebSocket protocol with
//! `model_id=scribe_v3_live`, `audio_format=pcm_16000` and `commit_strategy=vad`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::audio::{Resampler, bytes_to_i16, i16_to_bytes, to_mono};
use crate::transcription;

pub const SOURCES: [&str; 2] = ["you", "others"];
const UNAVAILABLE: &str = "Live transcription unavailable. Check ElevenLabs connection, quota and API key.";

/// Server message types that are treated as errors.
const ERROR_EVENTS: [&str; 15] = [
    "error",
    "auth_error",
    "quota_exceeded",
    "commit_throttled",
    "transcriber_error",
    "unaccepted_terms",
    "unaccepted_terms_error",
    "rate_limited",
    "input_error",
    "invalid_request",
    "queue_overflow",
    "resource_exhausted",
    "session_time_limit_exceeded",
    "chunk_size_exceeded",
    "insufficient_audio_activity",
];

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Segment {
    pub source: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SourceState {
    pub status: String,
    pub partial: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Default)]
pub struct LiveSnapshot {
    pub segments: Vec<Segment>,
    pub sources: BTreeMap<String, SourceState>,
}

struct Inner {
    segments: VecDeque<Segment>,
    sources: BTreeMap<String, SourceState>,
}

type Chunk = (Vec<u8>, u32, u16);

pub struct LiveTranscript {
    inner: Arc<Mutex<Inner>>,
    stop: Arc<AtomicBool>,
    senders: BTreeMap<String, SyncSender<Chunk>>,
    receivers: Mutex<Option<Vec<(String, Receiver<Chunk>)>>>,
}

impl LiveTranscript {
    pub fn new() -> Self {
        let mut senders = BTreeMap::new();
        let mut receivers = Vec::new();
        let mut sources = BTreeMap::new();
        for source in SOURCES {
            let (tx, rx) = sync_channel(250);
            senders.insert(source.to_owned(), tx);
            receivers.push((source.to_owned(), rx));
            sources.insert(source.to_owned(), SourceState { status: "connecting".into(), partial: String::new(), error: None });
        }
        LiveTranscript {
            inner: Arc::new(Mutex::new(Inner { segments: VecDeque::new(), sources })),
            stop: Arc::new(AtomicBool::new(false)),
            senders,
            receivers: Mutex::new(Some(receivers)),
        }
    }

    pub fn start(&self) -> std::io::Result<()> {
        let receivers = self.receivers.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
        let inner = self.inner.clone();
        let stop = self.stop.clone();
        std::thread::Builder::new().name("live-transcript".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(_) => {
                    for (source, _) in &receivers {
                        update(&inner, source, |s| {
                            s.status = "error".into();
                            s.error = Some(UNAVAILABLE.into());
                        });
                    }
                    return;
                }
            };
            runtime.block_on(async move {
                let tasks = receivers.into_iter().map(|(source, rx)| stream(inner.clone(), stop.clone(), source, rx));
                futures_util::future::join_all(tasks).await;
            });
        })?;
        Ok(())
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> LiveSnapshot {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        LiveSnapshot { segments: inner.segments.iter().cloned().collect(), sources: inner.sources.clone() }
    }

    pub fn feed(&self, source: &str, data: &[u8], rate: u32, channels: u16) {
        let errored = self.inner.lock().unwrap_or_else(|e| e.into_inner()).sources.get(source).is_some_and(|s| s.status == "error");
        if self.stop.load(Ordering::SeqCst) || errored {
            return;
        }
        let Some(tx) = self.senders.get(source) else { return };
        if let Err(TrySendError::Full(_)) = tx.try_send((data.to_vec(), rate, channels)) {
            update(&self.inner, source, |s| {
                s.status = "error".into();
                s.error = Some("Live audio fell behind. Restart recording to reconnect.".into());
            });
        }
    }
}

impl Default for LiveTranscript {
    fn default() -> Self {
        Self::new()
    }
}

fn update(inner: &Arc<Mutex<Inner>>, source: &str, f: impl FnOnce(&mut SourceState)) {
    let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = inner.sources.get_mut(source) {
        f(s);
    }
}

fn status_of(inner: &Arc<Mutex<Inner>>, source: &str) -> String {
    inner.lock().unwrap_or_else(|e| e.into_inner()).sources.get(source).map(|s| s.status.clone()).unwrap_or_default()
}

fn on_message(inner: &Arc<Mutex<Inner>>, source: &str, data: &Value) {
    let kind = data.get("message_type").and_then(Value::as_str).unwrap_or("");
    let text = data.get("text").and_then(Value::as_str).unwrap_or("").to_owned();
    match kind {
        "partial_transcript" => update(inner, source, |s| s.partial = text),
        "committed_transcript" => {
            let mut guard = inner.lock().unwrap_or_else(|e| e.into_inner());
            if !text.trim().is_empty() {
                guard.segments.push_back(Segment { source: source.to_owned(), text });
                while guard.segments.len() > 500 {
                    guard.segments.pop_front();
                }
            }
            if let Some(s) = guard.sources.get_mut(source) {
                s.partial.clear();
            }
        }
        k if ERROR_EVENTS.contains(&k) => update(inner, source, |s| {
            s.status = "error".into();
            s.error = Some(UNAVAILABLE.into());
        }),
        _ => {}
    }
}

pub fn websocket_url() -> String {
    let base = transcription::BASE_URL.replacen("https://", "wss://", 1);
    format!("{base}/v1/speech-to-text/realtime?model_id=scribe_v3_live&audio_format=pcm_16000&commit_strategy=vad&include_timestamps=false")
}

async fn stream(inner: Arc<Mutex<Inner>>, stop: Arc<AtomicBool>, source: String, rx: Receiver<Chunk>) {
    let result = stream_inner(&inner, &stop, &source, &rx).await;
    if result.is_err() {
        update(&inner, &source, |s| {
            s.status = "error".into();
            s.error = Some(UNAVAILABLE.into());
        });
    }
    if stop.load(Ordering::SeqCst) {
        update(&inner, &source, |s| {
            s.status = "stopped".into();
            s.partial.clear();
        });
    }
}

async fn stream_inner(inner: &Arc<Mutex<Inner>>, stop: &Arc<AtomicBool>, source: &str, rx: &Receiver<Chunk>) -> Result<(), String> {
    let key = transcription::api_key()?;
    let mut request = websocket_url().into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert("xi-api-key", key.parse().map_err(|_| "invalid key".to_owned())?);
    let (ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(request))
        .await
        .map_err(|_| "timeout".to_owned())?
        .map_err(|e| e.to_string())?;
    let (mut sink, mut incoming) = ws.split();

    // Message handler: reports an error on failure and a close when the socket ends.
    let reader_inner = inner.clone();
    let reader_stop = stop.clone();
    let reader_source = source.to_owned();
    let reader = tokio::spawn(async move {
        while let Some(message) = incoming.next().await {
            match message {
                Ok(Message::Text(text)) => match serde_json::from_str::<Value>(text.as_str()) {
                    Ok(data) => on_message(&reader_inner, &reader_source, &data),
                    Err(_) => on_message(&reader_inner, &reader_source, &json!({"message_type": "error"})),
                },
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => {
                    on_message(&reader_inner, &reader_source, &json!({"message_type": "error"}));
                    break;
                }
            }
        }
        let stopped = reader_stop.load(Ordering::SeqCst);
        update(&reader_inner, &reader_source, |s| {
            s.status = if stopped { "stopped".into() } else { "error".into() };
            s.error = if stopped { None } else { Some("Live connection closed. Restart recording to reconnect.".into()) };
        });
    });

    update(inner, source, |s| s.status = "listening".into());
    let mut resampler: Option<(u32, Resampler)> = None;
    let mut buffer: Vec<u8> = Vec::new();
    let result: Result<(), String> = async {
        while !stop.load(Ordering::SeqCst) {
            if status_of(inner, source) == "error" {
                break;
            }
            let (data, rate, channels) = match rx.try_recv() {
                Ok(chunk) => chunk,
                Err(TryRecvError::Empty) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }
                Err(TryRecvError::Disconnected) => break,
            };
            let mono = to_mono(&bytes_to_i16(&data), channels as usize);
            if resampler.as_ref().is_none_or(|(r, _)| *r != rate) {
                resampler = Some((rate, Resampler::new(rate, 16000, 1)));
            }
            let converted = resampler.as_mut().map(|(_, r)| r.process(&mono)).unwrap_or_default();
            buffer.extend_from_slice(&i16_to_bytes(&converted));
            if buffer.len() >= 6400 {
                // At least 200 ms of mono PCM16.
                let message = json!({
                    "message_type": "input_audio_chunk",
                    "audio_base_64": base64::engine::general_purpose::STANDARD.encode(&buffer),
                    "commit": false,
                    "sample_rate": 16000,
                    "previous_text": null,
                });
                tokio::time::timeout(Duration::from_secs(3), sink.send(Message::Text(message.to_string().into())))
                    .await
                    .map_err(|_| "send timeout".to_owned())?
                    .map_err(|e| e.to_string())?;
                buffer.clear();
            }
        }
        // Preview is discarded on stop; the full saved audio is transcribed separately.
        Ok(())
    }
    .await;
    let close = Message::Close(Some(CloseFrame { code: CloseCode::Normal, reason: "User ended conversation".into() }));
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = sink.send(close).await;
        let _ = sink.close().await;
    })
    .await;
    reader.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_events() {
        let live = LiveTranscript::new();
        on_message(&live.inner, "you", &json!({"message_type": "partial_transcript", "text": "hel"}));
        assert_eq!(live.snapshot().sources["you"].partial, "hel");
        on_message(&live.inner, "you", &json!({"message_type": "committed_transcript", "text": "hello"}));
        let snap = live.snapshot();
        assert_eq!(snap.segments, vec![Segment { source: "you".into(), text: "hello".into() }]);
        assert_eq!(snap.sources["you"].partial, "");
        on_message(&live.inner, "others", &json!({"message_type": "quota_exceeded"}));
        assert_eq!(live.snapshot().sources["others"].status, "error");
        assert!(websocket_url().starts_with("wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v3_live"));
    }
}
