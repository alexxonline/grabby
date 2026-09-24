import threading
import logging
import wave
import audioop
import subprocess
import uuid
import json
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
        self._save_pending = False
        self._saved_output: Path | None = None
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

    @property
    def error(self) -> str | None:
        return self._error

    @property
    def save_pending(self) -> bool:
        return self._save_pending

    @property
    def is_saving(self) -> bool:
        return self._saving

    def _capture_failed(self, exc: Exception):
        self._error = f"Audio capture failed: {exc}"
        self._recording = False
        if self._capture_dir:
            self._save_pending = any(
                path.exists() and path.stat().st_size >= channels * 2
                for path, channels in ((self._capture_dir / "loopback.pcm", self._lb_channels),
                                       (self._capture_dir / "mic.pcm", self._mic_channels)))
        if self.live:
            try:
                self.live.stop()
            except Exception:
                log.exception("Failed to stop live transcription after capture failure")

    # ------------------------------------------------------------------
    def _write_manifest(self):
        manifest = self._capture_dir / "capture.json"
        temporary = manifest.with_suffix(".tmp")
        temporary.write_text(json.dumps({
            "name": self._current_file, "notes_id": self.notes_id,
            "lb_rate": self._lb_rate, "lb_channels": self._lb_channels,
            "mic_rate": self._mic_rate, "mic_channels": self._mic_channels,
            "saved_output": self._saved_output.name if self._saved_output else None,
        }), encoding="utf-8")
        temporary.replace(manifest)

    def recover_pending(self):
        """Restore one interrupted capture at a time, without guessing raw PCM formats."""
        with self._lock:
            if self._recording or self._saving or self._save_pending or (self._thread and self._thread.is_alive()):
                return
            for manifest in sorted(RECORDINGS_DIR.glob(".capture-*/capture.json")):
                try:
                    data = json.loads(manifest.read_text(encoding="utf-8"))
                    name = self._validate_stem(data["name"])
                    params = [data[k] for k in ("lb_rate", "lb_channels", "mic_rate", "mic_channels")]
                    if any(not isinstance(value, int) or value <= 0 for value in params):
                        continue
                    saved = data.get("saved_output")
                    if saved and (Path(saved).name != saved or saved not in (name + ".wav", name + ".mp3")):
                        continue
                    if not any(p.stat().st_size for p in manifest.parent.glob("*.pcm")):
                        continue
                    self._capture_dir = manifest.parent
                    self._current_file = name
                    self.notes_id = data["notes_id"]
                    self._lb_rate, self._lb_channels, self._mic_rate, self._mic_channels = params
                    self._saved_output = RECORDINGS_DIR / saved if saved else None
                    self._save_pending = True
                    self._error = "Interrupted audio capture recovered. Use Retry Save."
                    return
                except (OSError, ValueError, KeyError, TypeError):
                    log.exception("Could not restore capture %s", manifest)

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
        self.recover_pending()
        with self._lock:
            if self._save_pending:
                raise RuntimeError("Save the previous recording before starting another. Use Retry Save.")
            if self._recording or self._saving or (self._thread and self._thread.is_alive()):
                raise RuntimeError("Already recording")

            started_at = datetime.now()
            if filename is None:
                filename = f"recording_{started_at.strftime('%Y%m%d_%H%M%S')}"
            filename = self._validate_stem(filename)
            RECORDINGS_DIR.mkdir(exist_ok=True)
            filename = self._unique_stem(filename)
            self._started_at = started_at
            self._current_file = filename
            self.notes_id = db.create_notes()["id"]
            self._capture_dir = RECORDINGS_DIR / f".capture-{uuid.uuid4().hex}"
            self._capture_dir.mkdir()
            self._error = None
            self._saved_output = None
            self._recording = True
            self.live = None
            try:
                self.live = LiveTranscript()
                self.live.start()
                # Publish and start the thread before Stop can acquire the lock.
                self._thread = threading.Thread(target=self._record_loop, daemon=True)
                self._thread.start()
            except Exception as exc:
                log.exception("Failed to start audio capture")
                self._capture_failed(exc)
                raise RuntimeError(self._error) from exc

    def stop(self) -> str:
        with self._lock:
            if self._saving:
                raise RuntimeError("Recording is already being saved")
            if not self._recording and not self._save_pending:
                raise RuntimeError(self._error or "Not recording")
            self._recording = False
            self._save_pending = True
            self._saving = True

        try:
            if self.live:
                self.live.stop()
            if self._thread:
                self._thread.join(timeout=10)
                if self._thread.is_alive():
                    raise RuntimeError("Audio capture is still stopping; please wait")
            output = self._mix_and_save()
            self._save_pending = False
            self._error = None
            return output
        except Exception as exc:
            log.exception("Saving recording failed; captured audio retained for retry")
            self._error = f"Could not save recording: {exc}. Resolve the problem and use Retry Save."
            raise RuntimeError(self._error) from exc
        finally:
            with self._lock:
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
            self._write_manifest()

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

                failures = []

                def read_source(stream, output, source, rate, channels):
                    try:
                        while self._recording:
                            # Each device is drained at its own rate. Stop on overflow
                            # instead of silently splicing a gap out of one stream.
                            try:
                                data = stream.read(1024, exception_on_overflow=True)
                            except Exception:
                                if not self._recording:
                                    return
                                raise
                            output.write(data)
                            if self.live:
                                self.live.feed(source, data, rate, channels)
                    except Exception as exc:
                        failures.append(exc)
                        self._recording = False

                readers = []
                try:
                    for args in streams:
                        reader = threading.Thread(target=read_source, args=args, daemon=True)
                        reader.start()
                        readers.append(reader)
                finally:
                    if len(readers) != len(streams):
                        self._recording = False
                    for reader in readers:
                        reader.join()
                if failures:
                    raise failures[0]
        except Exception as exc:
            log.exception("Audio capture failed")
            self._capture_failed(exc)
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
        if not path.exists():
            return
        with path.open("rb") as source:
            while data := source.read(4096 * channels * 2):
                data = data[:len(data) // (channels * 2) * (channels * 2)]
                if not data:
                    break
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
        if self._saved_output is None:
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
            # If attaching notes fails, retry it without exporting a second file.
            self._saved_output = output_file
            self._write_manifest()

        output_file = self._saved_output
        db.attach_notes(self.notes_id, output_file.name)
        # Cleanup failure must not turn an already saved recording into a failed save.
        try:
            # Retire recovery first: leftover PCM after a cleanup failure must
            # not make a successfully saved meeting appear interrupted again.
            for name in ("capture.json", "capture.tmp", "loopback.pcm", "mic.pcm", "mixed.wav", "mixed.mp3"):
                (self._capture_dir / name).unlink(missing_ok=True)
            self._capture_dir.rmdir()
        except OSError:
            log.warning("Recording saved, but temporary audio cleanup failed", exc_info=True)
        self._capture_dir = None
        return str(output_file)


# Singleton instance
recorder = MeetingRecorder()
