"""Notes persist independently of audio format and transcript processing."""
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from fastapi import HTTPException
from app import database as db

with patch.object(db, "init_db"):
    from app import main
from app.recorder import MeetingRecorder


class NotesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.patches = [patch.object(db, "DB_PATH", self.root / "test.db"),
                        patch.object(db, "_local", threading.local()),
                        patch.object(main, "RECORDINGS_DIR", self.root),
                        patch("app.recorder.RECORDINGS_DIR", self.root)]
        for item in self.patches:
            item.start()
        db.init_db()

    def tearDown(self):
        db._get_conn().close()
        for item in reversed(self.patches):
            item.stop()
        self.temp.cleanup()

    def test_active_notes_follow_both_export_formats(self):
        for fallback in (False, True):
            with self.subTest(fallback=fallback):
                recorder = MeetingRecorder()
                with patch("app.recorder.LiveTranscript"), patch("app.recorder.threading.Thread"):
                    recorder.start(f"meeting_{fallback}")
                recorder.live.snapshot.return_value = {}
                with patch.object(main, "recorder", recorder):
                    self.assertEqual(main.get_status().notes_id, recorder.notes_id)
                note = "Decisions\n• Follow up with José"
                main.update_notes(recorder.notes_id, main.UpdateTranscriptionRequest(text=note))
                (recorder._capture_dir / "loopback.pcm").write_bytes(b"\0" * 4096)
                (recorder._capture_dir / "mic.pcm").write_bytes(b"\0" * 2048)

                def export(source, output):
                    if fallback:
                        raise FileNotFoundError("ffmpeg unavailable")
                    output.write_bytes(b"audio")

                with patch.object(recorder, "_export_mp3", side_effect=export):
                    filename = Path(recorder._mix_and_save()).name
                self.assertTrue(filename.endswith(".wav" if fallback else ".mp3"))
                self.assertEqual(main.get_recording_notes(filename), {"id": recorder.notes_id, "text": note})
                # A save already in flight at Stop still targets the same notes.
                main.update_notes(recorder.notes_id, main.UpdateTranscriptionRequest(text="After stopping"))
                self.assertEqual(main.get_recording_notes(filename)["text"], "After stopping")

    def test_existing_recordings_persistence_retranscription_and_cleanup(self):
        filename = "existing.wav"
        audio = self.root / filename
        audio.write_bytes(b"audio")
        notes = main.get_recording_notes(filename)
        self.assertEqual(notes["text"], "")
        main.update_notes(notes["id"], main.UpdateTranscriptionRequest(text="Action items"))
        db._get_conn().close()
        db._local.conn = None
        db.init_db()
        self.assertEqual(main.get_recording_notes(filename)["text"], "Action items")
        db.save_transcription(filename, "First transcript")
        db.save_transcription(filename, "New transcript")
        audio.unlink()
        main.rename_recording(filename, main.RenameRequest(new_name="New title"))
        self.assertEqual(main.get_recording_notes(filename)["text"], "Action items")
        main.update_notes(notes["id"], main.UpdateTranscriptionRequest(text=""))
        self.assertEqual(main.get_recording_notes(filename)["text"], "")
        main.delete_recording(filename)
        self.assertIsNone(db.get_notes(notes["id"]))

    def test_missing_recording_or_notes_cannot_be_written(self):
        with self.assertRaises(HTTPException) as caught:
            main.get_recording_notes("missing.wav")
        self.assertEqual(caught.exception.status_code, 404)
        with self.assertRaises(HTTPException) as caught:
            main.update_notes("missing", main.UpdateTranscriptionRequest(text="Hello"))
        self.assertEqual(caught.exception.status_code, 404)

    def test_distinct_recordings_have_independent_notes(self):
        for filename in ("same.wav", "same.mp3"):
            (self.root / filename).write_bytes(b"audio")
        first = main.get_recording_notes("same.wav")
        second = main.get_recording_notes("same.mp3")
        self.assertNotEqual(first["id"], second["id"])
        main.update_notes(first["id"], main.UpdateTranscriptionRequest(text="Only WAV"))
        self.assertEqual(main.get_notes(second["id"])["text"], "")
