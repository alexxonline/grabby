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

    def test_stop_failure_can_retry_without_losing_audio_or_notes(self):
        from fastapi import BackgroundTasks, HTTPException
        from app import main

        recorder = self.recorder
        recorder._recording = True
        self.write_pcm("loopback.pcm", [1000] * 100)
        self.write_pcm("mic.pcm", [2000] * 100)
        capture = recorder._capture_dir
        with patch.object(main, "recorder", recorder), \
                patch("app.recorder.RECORDINGS_DIR", self.root), \
                patch("app.recorder.db.attach_notes") as attach:
            tasks = BackgroundTasks()
            with patch.object(recorder, "_mix_to_wav", side_effect=OSError("disk full")), \
                    self.assertLogs("app.recorder", level="ERROR"), \
                    self.assertRaises(HTTPException):
                main.stop_recording(tasks)
            self.assertEqual(tasks.tasks, [])
            self.assertFalse(recorder.is_recording)
            self.assertTrue(main.get_status().save_pending)
            self.assertFalse(main.get_status().saving)
            self.assertIn("Retry Save", recorder.error)
            self.assertTrue((capture / "loopback.pcm").exists())
            with self.assertRaisesRegex(RuntimeError, "previous recording"):
                recorder.start("next")
            with patch.object(recorder, "_export_mp3", side_effect=FileNotFoundError()), \
                    self.assertLogs("app.recorder", level="WARNING"):
                result = main.stop_recording(tasks)
            self.assertEqual(result.filename, "meeting.wav")
            self.assertEqual(len(tasks.tasks), 1)
            self.assertFalse(recorder.save_pending)
            self.assertIsNone(recorder.error)
            attach.assert_called_once_with("notes", "meeting.wav")
            self.assertFalse(capture.exists())
            main._transcribing.pop("meeting.wav", None)

    def test_notes_failure_retries_existing_export(self):
        recorder = self.recorder
        recorder._recording = True
        self.write_pcm("loopback.pcm", [1000] * 100)
        self.write_pcm("mic.pcm", [2000] * 100)

        def export(source, output):
            output.write_bytes(b"encoded audio")

        with patch("app.recorder.RECORDINGS_DIR", self.root), \
                patch.object(recorder, "_export_mp3", side_effect=export) as encode, \
                patch("app.recorder.db.attach_notes", side_effect=[RuntimeError("database locked"), None]):
            with self.assertLogs("app.recorder", level="ERROR"), self.assertRaises(RuntimeError):
                recorder.stop()
            self.assertTrue(recorder.save_pending)
            self.assertEqual((self.root / "meeting.mp3").read_bytes(), b"encoded audio")
            output = recorder.stop()
            encode.assert_called_once()
            self.assertEqual(Path(output).name, "meeting.mp3")
            self.assertFalse(recorder.save_pending)

    def test_cleanup_failure_does_not_fail_saved_recording(self):
        recorder = self.recorder
        recorder._recording = True
        self.write_pcm("loopback.pcm", [1000] * 100)
        self.write_pcm("mic.pcm", [2000] * 100)
        with patch("app.recorder.RECORDINGS_DIR", self.root), \
                patch("app.recorder.db.attach_notes"), \
                patch.object(recorder, "_export_mp3", side_effect=FileNotFoundError()), \
                patch.object(Path, "unlink", side_effect=PermissionError("locked")), \
                self.assertLogs("app.recorder", level="WARNING"):
            output = recorder.stop()
        self.assertTrue(Path(output).exists())
        self.assertFalse(recorder.save_pending)
        self.assertIsNone(recorder.error)

    def test_still_stopping_capture_can_be_saved_later(self):
        recorder = self.recorder
        recorder._recording = True
        recorder._thread = MagicMock()
        recorder._thread.is_alive.return_value = True
        with patch.object(recorder, "_mix_and_save", return_value="meeting.wav") as save:
            with self.assertLogs("app.recorder", level="ERROR"), self.assertRaises(RuntimeError):
                recorder.stop()
            save.assert_not_called()
            self.assertTrue(recorder.save_pending)
            recorder._thread.is_alive.return_value = False
            self.assertEqual(recorder.stop(), "meeting.wav")
            save.assert_called_once()

    def test_concurrent_save_is_rejected(self):
        self.recorder._save_pending = True
        self.recorder._saving = True
        with patch.object(self.recorder, "_mix_and_save") as save:
            with self.assertRaisesRegex(RuntimeError, "already being saved"):
                self.recorder.stop()
            save.assert_not_called()

    def test_audio_initialization_failure_is_visible_in_status_and_stop(self):
        from app import main

        recorder = self.recorder
        recorder._recording = True
        recorder.live = MagicMock()
        with patch("app.recorder.pyaudio.PyAudio", side_effect=OSError("device unavailable")), \
                self.assertLogs("app.recorder", level="ERROR"):
            recorder._record_loop()
        self.assertFalse(recorder.is_recording)
        recorder.live.stop.assert_called_once()
        with patch.object(main, "recorder", recorder):
            status = main.get_status()
            self.assertFalse(status.recording)
            self.assertIn("device unavailable", status.error)
        with self.assertRaisesRegex(RuntimeError, "device unavailable"):
            recorder.stop()

    def test_partial_audio_initialization_releases_open_stream(self):
        recorder = self.recorder
        recorder._recording = True
        recorder.live = MagicMock()
        backend, stream = MagicMock(), MagicMock()
        device = {"defaultSampleRate": 16000, "maxInputChannels": 1, "index": 0}
        backend.open.side_effect = [stream, OSError("microphone unavailable")]
        with patch("app.recorder.pyaudio.PyAudio", return_value=backend), \
                patch.object(recorder, "_get_loopback_device", return_value=device), \
                patch.object(recorder, "_get_default_mic", return_value=device), \
                self.assertLogs("app.recorder", level="ERROR"):
            recorder._record_loop()
        self.assertFalse(recorder.is_recording)
        self.assertIn("microphone unavailable", recorder.error)
        stream.stop_stream.assert_called_once()
        stream.close.assert_called_once()
        backend.terminate.assert_called_once()
        recorder.live.stop.assert_called_once()

    def test_thread_start_failures_reset_state_and_allow_retry(self):
        for failure in ("live", "capture"):
            with self.subTest(failure=failure), \
                    patch("app.recorder.RECORDINGS_DIR", self.root), \
                    patch("app.recorder.db.get_transcription", return_value=None), \
                    patch("app.recorder.db.create_notes", return_value={"id": "notes"}), \
                    patch("app.recorder.LiveTranscript") as live, \
                    patch("app.recorder.threading.Thread") as thread:
                recorder = MeetingRecorder()
                thread.return_value.is_alive.return_value = False
                failing_start = live.return_value.start if failure == "live" else thread.return_value.start
                failing_start.side_effect = RuntimeError("cannot start thread")
                with self.assertLogs("app.recorder", level="ERROR"), \
                        self.assertRaisesRegex(RuntimeError, "cannot start thread"):
                    recorder.start("failed")
                self.assertFalse(recorder.is_recording)
                live.return_value.stop.assert_called_once()
                failing_start.side_effect = None
                recorder.start("retry")
                self.assertTrue(recorder.is_recording)
                self.assertIsNone(recorder.error)

    def test_start_response_does_not_claim_recording_after_immediate_failure(self):
        from app import main

        with patch.object(main, "recorder", self.recorder), \
                patch.object(self.recorder, "start"), \
                patch.object(self.recorder, "_error", "Audio capture failed: no device"):
            status = main.start_recording()
        self.assertFalse(status.recording)
        self.assertIn("no device", status.error)

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
