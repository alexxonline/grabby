"""ElevenLabs Scribe v2 transcription service."""

import os
import logging
from pathlib import Path

from elevenlabs.client import ElevenLabs

log = logging.getLogger(__name__)


def get_client() -> ElevenLabs:
    api_key = os.getenv("ELEVENLABS_API_KEY")
    if not api_key:
        raise RuntimeError(
            "ELEVENLABS_API_KEY environment variable is not set. "
            "Get your key from https://elevenlabs.io/app/settings/api-keys"
        )
    return ElevenLabs(api_key=api_key)


def transcribe_file(filepath: Path, language_code: str | None = None) -> dict:
    """
    Transcribe an audio file using ElevenLabs Scribe v2 with speaker diarization.

    Returns:
        dict with keys:
          - text: formatted "Speaker N: ..." text
          - language: detected language code
          - raw_words: list of word-level data from API
    """
    client = get_client()

    with open(filepath, "rb") as f:
        transcription = client.speech_to_text.convert(
            file=f,
            model_id="scribe_v2",
            tag_audio_events=False,
            language_code=language_code,
            diarize=True,
        )

    # Build speaker-diarized text from word-level data
    formatted = _format_diarized(transcription)

    # Detect language from response
    detected_lang = getattr(transcription, "language_code", None)

    return {
        "text": formatted,
        "language": detected_lang,
    }


def _format_diarized(transcription) -> str:
    """
    Format transcription with speaker labels.
    Groups consecutive words by the same speaker into paragraphs.
    """
    words = getattr(transcription, "words", None)
    if not words:
        # Fallback: return plain text if no word-level data
        return getattr(transcription, "text", "")

    segments: list[dict] = []
    current_speaker = None
    current_text_parts: list[str] = []

    for word in words:
        speaker = getattr(word, "speaker_id", None)
        text = getattr(word, "text", "")

        if speaker != current_speaker and current_text_parts:
            segments.append({
                "speaker": current_speaker,
                "text": "".join(current_text_parts).strip(),
            })
            current_text_parts = []

        current_speaker = speaker
        current_text_parts.append(text)

    # Don't forget the last segment
    if current_text_parts:
        segments.append({
            "speaker": current_speaker,
            "text": "".join(current_text_parts).strip(),
        })

    # Build speaker label mapping (speaker_0 -> Speaker 1, etc.)
    speaker_map: dict[str | None, str] = {}
    counter = 1
    for seg in segments:
        sp = seg["speaker"]
        if sp not in speaker_map:
            speaker_map[sp] = f"Speaker {counter}"
            counter += 1

    # Format output
    lines = []
    for seg in segments:
        label = speaker_map[seg["speaker"]]
        lines.append(f"{label}: {seg['text']}")

    return "\n".join(lines)
