import threading
import logging
import wave
import audioop
import subprocess
import uuid
from contextlib import ExitStack
from itertools import zip_longest
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
        self._capture_dir: Path | None = None
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
            self._capture_dir = RECORDINGS_DIR / f".capture-{uuid.uuid4().hex}"
            self._capture_dir.mkdir()
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
        # Raw PCM avoids both unbounded RAM and WAV's 4 GiB capture limit.
        p = None
        streams = []
        try:
            p = pyaudio.PyAudio()
            loopback_dev = self._get_loopback_device(p)
            mic_dev = self._get_default_mic(p)
            if loopback_dev is None:
                raise RuntimeError("No WASAPI loopback device found")

            self._lb_rate = int(loopback_dev["defaultSampleRate"])
            self._lb_channels = loopback_dev["maxInputChannels"]
            self._mic_rate = int(mic_dev["defaultSampleRate"])
            self._mic_channels = mic_dev["maxInputChannels"] or 1

            with ExitStack() as files:
                for source, device, rate, channels, name in (
                    ("others", loopback_dev, self._lb_rate, self._lb_channels, "loopback.pcm"),
                    ("you", mic_dev, self._mic_rate, self._mic_channels, "mic.pcm"),
                ):
                    output = files.enter_context((self._capture_dir / name).open("wb"))
                    stream = p.open(format=pyaudio.paInt16, channels=channels, rate=rate,
                                    input=True, input_device_index=device["index"],
                                    frames_per_buffer=1024)
                    streams.append((stream, output, source, rate, channels))

                while self._recording:
                    for stream, output, source, rate, channels in streams:
                        try:
                            data = stream.read(1024, exception_on_overflow=False)
                        except Exception:
                            if not self._recording:
                                break
                            raise
                        # Disk errors must stop capture, not silently drop audio.
                        output.write(data)
                        if self.live:
                            self.live.feed(source, data, rate, channels)
        except Exception as exc:
            log.exception("Audio capture failed")
            self._error = f"Audio capture failed: {exc}"
            self._recording = False
            if self.live:
                self.live.stop()
        finally:
            for stream, *_ in streams:
                try:
                    stream.stop_stream()
                except Exception:
                    log.exception("Failed to stop audio stream")
                try:
                    stream.close()
                except Exception:
                    log.exception("Failed to close audio stream")
            if p is not None:
                try:
                    p.terminate()
                except Exception:
                    log.exception("Failed to terminate audio capture")

    @staticmethod
    def _pcm_chunks(path: Path, rate: int, channels: int, target_rate: int,
                    target_channels: int):
        """Yield aligned PCM blocks, preserving resampler state across reads."""
        block_bytes = 4096 * target_channels * 2
        pending = bytearray()
        state = None
        with path.open("rb") as source:
            while data := source.read(4096 * channels * 2):
                if rate != target_rate:
                    data, state = audioop.ratecv(data, 2, channels, rate, target_rate, state)
                if channels != target_channels:
                    segment = AudioSegment(data=data, sample_width=2, frame_rate=target_rate,
                                           channels=channels)
                    # Pydub supports mono <-> multichannel; bridge other layouts via mono.
                    if channels != 1 and target_channels != 1:
                        segment = segment.set_channels(1)
                    data = segment.set_channels(target_channels).raw_data
                pending.extend(data)
                while len(pending) >= block_bytes:
                    yield bytes(pending[:block_bytes])
                    del pending[:block_bytes]
            if pending:
                yield bytes(pending)

    def _mix_to_wav(self, output: Path):
        system = self._pcm_chunks(self._capture_dir / "loopback.pcm", self._lb_rate,
                                  self._lb_channels, self._lb_rate, self._lb_channels)
        mic = self._pcm_chunks(self._capture_dir / "mic.pcm", self._mic_rate,
                               self._mic_channels, self._lb_rate, self._lb_channels)
        try:
            with wave.open(str(output), "wb") as mixed:
                mixed.setnchannels(self._lb_channels)
                mixed.setsampwidth(2)
                mixed.setframerate(self._lb_rate)
                for system_data, mic_data in zip_longest(system, mic, fillvalue=b""):
                    size = max(len(system_data), len(mic_data))
                    mixed.writeframesraw(audioop.add(system_data.ljust(size, b"\0"),
                                                     mic_data.ljust(size, b"\0"), 2))
        finally:
            system.close()
            mic.close()

    def _export_mp3(self, source: Path, output: Path):
        # Never use AudioSegment.from_wav/export here: they load the whole meeting.
        subprocess.run(
            [AudioSegment.converter, "-nostdin", "-y", "-loglevel", "error",
             "-i", str(source), "-b:a", "128k", str(output)],
            check=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )

    def _mix_and_save(self) -> str:
        if self._capture_dir is None:
            raise RuntimeError("No captured audio to save")
        # Keep intermediate and incomplete exports out of the recordings listing.
        mixed_wav = self._capture_dir / "mixed.wav"
        encoded_mp3 = self._capture_dir / "mixed.mp3"
        self._mix_to_wav(mixed_wav)
        try:
            self._export_mp3(mixed_wav, encoded_mp3)
        except (OSError, subprocess.SubprocessError):
            log.warning("MP3 export unavailable; keeping WAV", exc_info=True)
            output_file = RECORDINGS_DIR / f"{self._current_file}.wav"
            mixed_wav.replace(output_file)
        else:
            output_file = RECORDINGS_DIR / f"{self._current_file}.mp3"
            encoded_mp3.replace(output_file)

        db.attach_notes(self.notes_id, output_file.name)
        # Preserve captured PCM on failures so the audio can be recovered.
        for name in ("loopback.pcm", "mic.pcm", "mixed.wav", "mixed.mp3"):
            (self._capture_dir / name).unlink(missing_ok=True)
        self._capture_dir.rmdir()
        self._capture_dir = None
        return str(output_file)


# Singleton instance
recorder = MeetingRecorder()
