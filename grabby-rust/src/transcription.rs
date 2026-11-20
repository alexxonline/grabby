//! ElevenLabs Scribe v3 transcription service.
//!
//! Sends the request the official ElevenLabs SDK (version 2.68.0) sends for
//! `model_id=scribe_v3`, `tag_audio_events=false` and `diarize=true`: a multipart POST to
//! `/v1/speech-to-text` with the `xi-api-key` header, a 240 s timeout and two retries on
//! 408/409/429/5xx.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::config;

pub const BASE_URL: &str = "https://api.elevenlabs.io";
const SDK_VERSION: &str = "2.68.0";
const MAX_RETRIES: u32 = 2;

pub fn api_key() -> Result<String, String> {
    match config::env_var("ELEVENLABS_API_KEY") {
        Some(key) if !key.is_empty() => Ok(key),
        _ => Err("No ElevenLabs API key is set. Add it in Grabby > Settings > API keys. \
                  Get your key from https://elevenlabs.io/app/settings/api-keys"
            .into()),
    }
}

#[derive(Debug, Deserialize)]
pub struct Word {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub speaker_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConvertResponse {
    #[serde(default)]
    pub language_code: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub words: Option<Vec<Word>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptResult {
    pub text: String,
    pub language: Option<String>,
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/x-wav",
        _ => "application/octet-stream",
    }
}

fn retry_delay(headers: &reqwest::header::HeaderMap, attempt: u32) -> Duration {
    if let Some(secs) = headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<f64>().ok()) {
        if secs > 0.0 {
            return Duration::from_secs_f64(secs.min(60.0));
        }
    }
    Duration::from_secs_f64((0.5 * 2f64.powi(attempt as i32)).min(60.0))
}

/// Transcribe an audio file using ElevenLabs Scribe v3 with speaker diarization.
pub fn transcribe_file(filepath: &Path, language_code: Option<&str>) -> Result<TranscriptResult, String> {
    let key = api_key()?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(240))
        .build()
        .map_err(|e| e.to_string())?;
    let file_name = filepath.file_name().and_then(|n| n.to_str()).unwrap_or("audio").to_owned();
    let mut attempt = 0;
    loop {
        let bytes = std::fs::read(filepath).map_err(|e| format!("[Errno 2] {e}: '{}'", filepath.display()))?;
        let part = reqwest::blocking::multipart::Part::bytes(bytes)
            .file_name(file_name.clone())
            .mime_str(mime_for(filepath))
            .map_err(|e| e.to_string())?;
        let mut form = reqwest::blocking::multipart::Form::new()
            .text("model_id", "scribe_v3")
            .text("tag_audio_events", "false")
            .text("diarize", "true");
        if let Some(lang) = language_code {
            form = form.text("language_code", lang.to_owned());
        }
        let form = form.part("file", part);
        let response = client
            .post(format!("{BASE_URL}/v1/speech-to-text"))
            .header("xi-api-key", &key)
            .header("User-Agent", format!("elevenlabs/{SDK_VERSION}"))
            .header("X-Fern-SDK-Name", "elevenlabs")
            .header("X-Fern-SDK-Version", SDK_VERSION)
            .multipart(form)
            .send()
            .map_err(|e| if e.is_timeout() { "The read operation timed out".to_owned() } else { e.to_string() })?;
        let status = response.status().as_u16();
        if (status >= 500 || matches!(status, 408 | 409 | 429)) && attempt < MAX_RETRIES {
            std::thread::sleep(retry_delay(response.headers(), attempt));
            attempt += 1;
            continue;
        }
        let body = response.text().map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) {
            return Err(format!("status_code: {status}, body: {body}"));
        }
        let parsed: ConvertResponse = serde_json::from_str(&body).map_err(|e| format!("status_code: {status}, body: {body} ({e})"))?;
        return Ok(TranscriptResult { text: format_diarized(&parsed), language: parsed.language_code });
    }
}

/// Group consecutive words by the same speaker into "Speaker N: ..." lines.
pub fn format_diarized(transcription: &ConvertResponse) -> String {
    let words = match &transcription.words {
        Some(w) if !w.is_empty() => w,
        // Fallback: plain text if no word-level data.
        _ => return transcription.text.clone(),
    };
    let mut segments: Vec<(Option<String>, String)> = Vec::new();
    let mut current_speaker: Option<String> = None;
    let mut parts = String::new();
    for word in words {
        if word.speaker_id != current_speaker && !parts.is_empty() {
            segments.push((current_speaker.clone(), parts.trim().to_owned()));
            parts.clear();
        }
        current_speaker = word.speaker_id.clone();
        parts.push_str(&word.text);
    }
    if !parts.is_empty() {
        segments.push((current_speaker, parts.trim().to_owned()));
    }
    let mut labels: Vec<(Option<String>, String)> = Vec::new();
    for (speaker, _) in &segments {
        if !labels.iter().any(|(s, _)| s == speaker) {
            let label = format!("Speaker {}", labels.len() + 1);
            labels.push((speaker.clone(), label));
        }
    }
    segments
        .iter()
        .map(|(speaker, text)| {
            let label = &labels.iter().find(|(s, _)| s == speaker).expect("every speaker has a label").1;
            format!("{label}: {text}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, speaker: Option<&str>) -> Word {
        Word { text: text.into(), speaker_id: speaker.map(Into::into) }
    }

    #[test]
    fn formats_speakers() {
        let r = ConvertResponse {
            language_code: Some("en".into()),
            text: "ignored".into(),
            words: Some(vec![
                w("Hello", Some("speaker_1")),
                w(" ", Some("speaker_1")),
                w("there", Some("speaker_1")),
                w(" ", Some("speaker_0")),
                w("Hi", Some("speaker_0")),
                w(" ", Some("speaker_1")),
                w("Again", Some("speaker_1")),
            ]),
        };
        assert_eq!(format_diarized(&r), "Speaker 1: Hello there\nSpeaker 2: Hi\nSpeaker 1: Again");
        let plain = ConvertResponse { language_code: None, text: "just text".into(), words: Some(vec![]) };
        assert_eq!(format_diarized(&plain), "just text");
    }
}
