import threading
import logging
import wave
import os
from datetime import datetime
from pathlib import Path

import pyaudiowpatch as pyaudio
from pydub import AudioSegment
from .realtime import LiveTranscript
from . import database as db

log = logging.getLogger(__name__)

RECORDINGS_DIR = Path(__file__).resolve().parent.parent / "recordings"
RECORDINGS_DIR.mkdir(exist_ok=True)


class MeetingRecorder:
    """Records system audio (loopback) and microphone, then mixes them."""

    def __init__(self):
        self._lock = threading.Lock()
        self._recording = False
        self._thread: threading.Thread | None = None
        self._loopback_frames: list[bytes] = []
        self._mic_frames: list[bytes] = []
        self._current_file: str | None = None
        self._started_at: datetime | None = None
        self._error: str | None = None
        self.live: LiveTranscript | None = None
        self._saving = False
        self.notes_id: str | None = None
        # Device params (set in _record_loop)
        self._lb_rate: int = 48000
        self._lb_channels: int = 2
        self._mic_rate: int = 44100
        self._mic_channels: int = 1

    # ------------------------------------------------------------------
    @property
    def is_recording(self) -> bool:
        return self._recording

    @property
    def current_file(self) -> str | None:
        return self._current_file

    @property
    def started_at(self) -> datetime | None:
        return self._started_at

    # ------------------------------------------------------------------
    def _get_loopback_device(self, p: pyaudio.PyAudio):
        """Find a WASAPI loopback device."""
        wasapi_info = p.get_host_api_info_by_type(pyaudio.paWASAPI)
        default_speakers = p.get_device_info_by_index(
            wasapi_info["defaultOutputDevice"]
        )
        # Try to find the loopback variant of the default output
        for i in range(p.get_device_count()):
            dev = p.get_device_info_by_index(i)
            if (
                dev["hostApi"] == wasapi_info["index"]
                and dev.get("name", "").startswith(default_speakers["name"])
                and dev["maxInputChannels"] > 0
                and "loopback" in dev["name"].lower()
            ):
                return dev
        # Fallback: any loopback
        for i in range(p.get_device_count()):
            dev = p.get_device_info_by_index(i)
            if (
                dev["hostApi"] == wasapi_info["index"]
                and "loopback" in dev["name"].lower()
            ):
                return dev
        return None

    def _get_default_mic(self, p: pyaudio.PyAudio):
        return p.get_default_input_device_info()

    # ------------------------------------------------------------------
    @staticmethod
    def _unique_stem(stem: str) -> str:
        """If a file with this stem already exists in RECORDINGS_DIR, append _1, _2, … until unique."""
        candidate = stem
        counter = 1
        while (any(path.stem.casefold() == candidate.casefold() for path in RECORDINGS_DIR.iterdir())
               or any(db.get_transcription(f"{candidate}{ext}") for ext in (".mp3", ".wav"))):
            candidate = f"{stem}_{counter}"
            counter += 1
        return candidate

    @staticmethod
    def _validate_stem(filename: str) -> str:
        filename = filename.removesuffix(".mp3").removesuffix(".wav")
        # Windows device names remain reserved even when followed by an extension.
        device = filename.split(".", 1)[0].rstrip(" ").upper()
        reserved = {"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"}
        reserved.update(f"{prefix}{digit}" for prefix in ("COM", "LPT") for digit in "123456789¹²³")
        if (not filename.strip() or len(filename) > 160
                or filename.endswith((".", " "))
                or any(ord(char) < 32 or char in '<>:"/\\|?*' for char in filename)
                or device in reserved
                or filename.casefold() in {"_temp_loopback", "_temp_mic"}):
            raise ValueError("Use a recording name of 1–160 characters without Windows filename restrictions or reserved names.")
        return filename

    def start(self, filename: str | None = None):
        with self._lock:
            if self._recording or self._saving or (self._thread and self._thread.is_alive()):
                raise RuntimeError("Already recording")

            started_at = datetime.now()
            if filename is None:
                filename = f"recording_{started_at.strftime('%Y%m%d_%H%M%S')}"
            filename = self._validate_stem(filename)
            filename = self._unique_stem(filename)
            self._started_at = started_at
            self._current_file = filename
            self.notes_id = db.create_notes()["id"]
            self._loopback_frames = []
            self._mic_frames = []
            self._error = None
            self._recording = True
            self.live = LiveTranscript()
            self.live.start()

        self._thread = threading.Thread(target=self._record_loop, daemon=True)
        self._thread.start()

    def stop(self) -> str:
        with self._lock:
            if not self._recording:
                raise RuntimeError("Not recording")
            self._recording = False
            self._saving = True
            if self.live:
                self.live.stop()

        try:
            if self._thread:
                self._thread.join(timeout=10)
                if self._thread.is_alive():
                    raise RuntimeError("Audio capture is still stopping; please wait")
            if self._error:
                raise RuntimeError(self._error)
            return self._mix_and_save()
        finally:
            self._saving = False

    # ------------------------------------------------------------------
    def _record_loop(self):
        p = pyaudio.PyAudio()
        try:
            loopback_dev = self._get_loopback_device(p)
            mic_dev = self._get_default_mic(p)

            if loopback_dev is None:
                self._error = "No WASAPI loopback device found"
                self._recording = False
                self.live.stop()
                p.terminate()
                return

            # Use each device's NATIVE sample rate and channels
            self._lb_rate = int(loopback_dev["defaultSampleRate"])
            self._lb_channels = loopback_dev["maxInputChannels"]
            self._mic_rate = int(mic_dev["defaultSampleRate"])
            self._mic_channels = mic_dev["maxInputChannels"] or 1

            log.info(
                "Loopback: %s @ %d Hz, %d ch | Mic: %s @ %d Hz, %d ch",
                loopback_dev["name"], self._lb_rate, self._lb_channels,
                mic_dev["name"], self._mic_rate, self._mic_channels,
            )

            loopback_stream = p.open(
                format=pyaudio.paInt16,
                channels=self._lb_channels,
                rate=self._lb_rate,
                input=True,
                input_device_index=loopback_dev["index"],
                frames_per_buffer=1024,
            )

            mic_stream = p.open(
                format=pyaudio.paInt16,
                channels=self._mic_channels,
                rate=self._mic_rate,
                input=True,
                input_device_index=mic_dev["index"],
                frames_per_buffer=1024,
            )
        except Exception as exc:
            log.exception("Failed to open audio streams")
            self._error = f"Failed to open audio streams: {exc}"
            self._recording = False
            self.live.stop()
            p.terminate()
            return

        try:
            while self._recording:
                try:
                    lb_data = loopback_stream.read(1024, exception_on_overflow=False)
                    self._loopback_frames.append(lb_data)
                    self.live.feed("others", lb_data, self._lb_rate, self._lb_channels)
                except Exception:
                    if not self._recording:
                        break

                try:
                    mic_data = mic_stream.read(1024, exception_on_overflow=False)
                    self._mic_frames.append(mic_data)
                    self.live.feed("you", mic_data, self._mic_rate, self._mic_channels)
                except Exception:
                    if not self._recording:
                        break
        finally:
            loopback_stream.stop_stream()
            loopback_stream.close()
            mic_stream.stop_stream()
            mic_stream.close()
            p.terminate()
            log.info(
                "Recording stopped. Loopback frames: %d, Mic frames: %d",
                len(self._loopback_frames), len(self._mic_frames),
            )

    # ------------------------------------------------------------------
    def _mix_and_save(self) -> str:
        RECORDINGS_DIR.mkdir(exist_ok=True)

        temp_lb = RECORDINGS_DIR / "_temp_loopback.wav"
        temp_mic = RECORDINGS_DIR / "_temp_mic.wav"

        log.info(
            "Saving: lb_frames=%d (rate=%d, ch=%d), mic_frames=%d (rate=%d, ch=%d)",
            len(self._loopback_frames), self._lb_rate, self._lb_channels,
            len(self._mic_frames), self._mic_rate, self._mic_channels,
        )

        # Save loopback WAV at its native rate/channels
        with wave.open(str(temp_lb), "wb") as wf:
            wf.setnchannels(self._lb_channels)
            wf.setsampwidth(2)
            wf.setframerate(self._lb_rate)
            wf.writeframes(b"".join(self._loopback_frames))

        # Save mic WAV at its native rate/channels
        with wave.open(str(temp_mic), "wb") as wf:
            wf.setnchannels(self._mic_channels)
            wf.setsampwidth(2)
            wf.setframerate(self._mic_rate)
            wf.writeframes(b"".join(self._mic_frames))

        # Load with pydub (it handles sample-rate via ffmpeg)
        system_audio = AudioSegment.from_wav(str(temp_lb))
        mic_audio = AudioSegment.from_wav(str(temp_mic))

        # Normalize both to same sample rate (use loopback rate as target)
        if mic_audio.frame_rate != system_audio.frame_rate:
            mic_audio = mic_audio.set_frame_rate(system_audio.frame_rate)

        # Match channel counts
        if mic_audio.channels != system_audio.channels:
            mic_audio = mic_audio.set_channels(system_audio.channels)

        # Overlay mic on system audio (use the longer as base)
        if len(system_audio) >= len(mic_audio):
            mixed = system_audio.overlay(mic_audio)
        else:
            mixed = mic_audio.overlay(system_audio)

        # Try MP3 (needs ffmpeg), fall back to WAV
        try:
            output_file = RECORDINGS_DIR / f"{self._current_file}.mp3"
            mixed.export(str(output_file), format="mp3", bitrate="128k")
        except Exception:
            output_file = RECORDINGS_DIR / f"{self._current_file}.wav"
            mixed.export(str(output_file), format="wav")

        # Cleanup temp files
        temp_lb.unlink(missing_ok=True)
        temp_mic.unlink(missing_ok=True)

        self._loopback_frames.clear()
        self._mic_frames.clear()

        db.attach_notes(self.notes_id, output_file.name)
        return str(output_file)


# Singleton instance
recorder = MeetingRecorder()
