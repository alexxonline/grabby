"""Disk-backed capture and bounded-memory export, without audio hardware."""
import audioop
import shutil
import tempfile
import tracemalloc
import unittest
import wave
from array import array
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from app.recorder import MeetingRecorder


class RecorderTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.recorder = MeetingRecorder()
        self.recorder._capture_dir = self.root / "capture"
        self.recorder._capture_dir.mkdir()
        self.recorder._current_file = "meeting"
        self.recorder.notes_id = "notes"
        self.recorder._lb_rate = self.recorder._mic_rate = 16000
        self.recorder._lb_channels = self.recorder._mic_channels = 1

    def write_pcm(self, name, samples):
        (self.recorder._capture_dir / name).write_bytes(array("h", samples).tobytes())

    def test_resampling_matches_continuous_conversion_and_preserves_longer_tail(self):
        self.recorder._mic_rate = 8000
        self.recorder._lb_channels = 2
        system = array("h", [1000, 2000] * 9000).tobytes()
        mic = array("h", [i % 1000 for i in range(6000)]).tobytes()
        (self.recorder._capture_dir / "loopback.pcm").write_bytes(system)
        (self.recorder._capture_dir / "mic.pcm").write_bytes(mic)
        output = self.root / "mixed.wav"
        self.recorder._mix_to_wav(output)
        converted, _ = audioop.ratecv(mic, 2, 1, 8000, 16000, None)
        converted = audioop.tostereo(converted, 2, 1, 1)
        expected = audioop.add(system.ljust(len(converted), b"\0"), converted, 2)
        with wave.open(str(output), "rb") as result:
            self.assertEqual(result.getnchannels(), 2)
            self.assertEqual(result.getframerate(), 16000)
            self.assertEqual(result.readframes(result.getnframes()), expected)

    def test_empty_source_and_clipping(self):
        for mic, expected in (([], [30000] * 5000), ([10000] * 5000, [32767] * 5000)):
            with self.subTest(empty=not mic):
                self.write_pcm("loopback.pcm", [30000] * 5000)
                self.write_pcm("mic.pcm", mic)
                output = self.root / "mixed.wav"
                self.recorder._mix_to_wav(output)
                with wave.open(str(output), "rb") as result:
                    self.assertEqual(array("h", result.readframes(6000)).tolist(), expected)

    def test_capture_writes_pcm_and_memory_does_not_grow_with_duration(self):
        recorder = self.recorder
        recorder.live = SimpleNamespace(feed=lambda *args: None, stop=lambda: None)
        device = {"defaultSampleRate": 16000, "maxInputChannels": 1, "index": 0}

        def capture(blocks):
            recorder._recording = True
            remaining = blocks

            class Stream:
                def read(self, frames, **kwargs):
                    nonlocal remaining
                    remaining -= 1
                    if remaining == 0:
                        recorder._recording = False
                    return b"\x01\x00" * frames

                def stop_stream(self):
                    pass

                def close(self):
                    pass

            backend = MagicMock()
            backend.open.return_value = Stream()
            with patch("app.recorder.pyaudio.PyAudio", return_value=backend), \
                    patch.object(recorder, "_get_loopback_device", return_value=device), \
                    patch.object(recorder, "_get_default_mic", return_value=device):
                tracemalloc.start()
                try:
                    recorder._record_loop()
                    peak = tracemalloc.get_traced_memory()[1]
                finally:
                    tracemalloc.stop()
            self.assertIsNone(recorder._error)
            self.assertEqual((recorder._capture_dir / "loopback.pcm").stat().st_size, blocks // 2 * 2048)
            self.assertEqual((recorder._capture_dir / "mic.pcm").stat().st_size, blocks // 2 * 2048)
            return peak

        short_peak = capture(200)
        long_peak = capture(20000)
        self.assertLess(long_peak, short_peak + 1024 * 1024)

    def test_mixing_memory_does_not_grow_with_duration(self):
        def mix(blocks):
            for name in ("loopback.pcm", "mic.pcm"):
                with (self.recorder._capture_dir / name).open("wb") as output:
                    for _ in range(blocks):
                        output.write(b"\0" * 8192)
            tracemalloc.start()
            try:
                self.recorder._mix_to_wav(self.root / "mixed.wav")
                return tracemalloc.get_traced_memory()[1]
            finally:
                tracemalloc.stop()

        short_peak = mix(20)
        long_peak = mix(2000)
        self.assertLess(long_peak, short_peak + 1024 * 1024)

    def test_disk_write_failure_stops_capture_and_releases_devices(self):
        recorder = self.recorder
        recorder._recording = True
        recorder.live = MagicMock()
        device = {"defaultSampleRate": 16000, "maxInputChannels": 1, "index": 0}
        backend = MagicMock()
        stream = backend.open.return_value
        stream.read.return_value = b"\0" * 2048
        output = MagicMock()
        output.__enter__.return_value = output
        output.write.side_effect = OSError("disk full")
        with patch("app.recorder.pyaudio.PyAudio", return_value=backend), \
                patch.object(recorder, "_get_loopback_device", return_value=device), \
                patch.object(recorder, "_get_default_mic", return_value=device), \
                patch.object(Path, "open", return_value=output), \
                self.assertLogs("app.recorder", level="ERROR"):
            recorder._record_loop()
        self.assertFalse(recorder.is_recording)
        self.assertIn("disk full", recorder._error)
        recorder.live.stop.assert_called_once()
        self.assertEqual(stream.close.call_count, 2)
        backend.terminate.assert_called_once()

    def test_failed_mp3_falls_back_and_cleans_partial_files(self):
        self.write_pcm("loopback.pcm", [1000] * 100)
        self.write_pcm("mic.pcm", [2000] * 100)
        capture_dir = self.recorder._capture_dir

        def fail(source, output):
            output.write_bytes(b"partial")
            raise OSError("encoder failed")

        with patch("app.recorder.RECORDINGS_DIR", self.root), \
                patch("app.recorder.db.attach_notes") as attach, \
                patch.object(self.recorder, "_export_mp3", side_effect=fail):
            output = Path(self.recorder._mix_and_save())
        self.assertEqual(output.suffix, ".wav")
        self.assertTrue(output.is_file())
        self.assertFalse(capture_dir.exists())
        attach.assert_called_once_with("notes", "meeting.wav")

    def test_failed_mix_preserves_capture(self):
        self.write_pcm("loopback.pcm", [1000] * 100)
        self.write_pcm("mic.pcm", [2000] * 100)
        with patch.object(self.recorder, "_mix_to_wav", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                self.recorder._mix_and_save()
        self.assertTrue((self.recorder._capture_dir / "loopback.pcm").exists())
        self.assertTrue((self.recorder._capture_dir / "mic.pcm").exists())

    @unittest.skipUnless(shutil.which("ffmpeg"), "FFmpeg unavailable")
    def test_real_mp3_export(self):
        self.write_pcm("loopback.pcm", [1000] * 16000)
        self.write_pcm("mic.pcm", [2000] * 16000)
        with patch("app.recorder.RECORDINGS_DIR", self.root), patch("app.recorder.db.attach_notes"):
            output = Path(self.recorder._mix_and_save())
        self.assertEqual(output.suffix, ".mp3")
        self.assertGreater(output.stat().st_size, 100)
