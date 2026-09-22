"""Filesystem boundaries and recording-name validation without real audio capture."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from fastapi import HTTPException
from app import database as db

with patch.object(db, "init_db"):
    from app import main
from app.recorder import MeetingRecorder


class SecurityTests(unittest.TestCase):
    def test_frontend_paths_stay_inside_dist(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "dist"
            root.mkdir()
            with patch.object(main, "FRONTEND_DIST", root):
                for path in ("../outside.txt", "..\\outside.txt", str(Path(temp) / "outside.txt")):
                    with self.subTest(path=path), self.assertRaises(HTTPException) as caught:
                        main._frontend_path(path)
                    self.assertEqual(caught.exception.status_code, 404)
                self.assertEqual(main._frontend_path("assets/app.js"), root / "assets" / "app.js")
                self.assertEqual(main._frontend_path("recordings"), root / "recordings")

    def test_invalid_names_do_not_start_capture_or_create_notes(self):
        names = ("../outside", "..\\outside", "C:\\outside", "meeting:today", "bad?name",
                 "bad*name", "bad|name", 'bad"name', "bad<name", "bad>name", "bad\x00name",
                 "", " ", ".", "..", ".mp3", "meeting.", "meeting ", "CON", "nul.wav",
                 "COM1", "LPT9.backup", "COM¹", "_temp_loopback", "_temp_mic", "x" * 161)
        with patch("app.recorder.LiveTranscript") as live, patch.object(db, "create_notes") as notes:
            for name in names:
                recorder = MeetingRecorder()
                with self.subTest(name=name), self.assertRaises(ValueError):
                    recorder.start(name)
                self.assertFalse(recorder.is_recording)
                self.assertIsNone(recorder.started_at)
            live.assert_not_called()
            notes.assert_not_called()

    def test_invalid_name_returns_bad_request(self):
        with patch.object(main, "recorder", MeetingRecorder()):
            with self.assertRaises(HTTPException) as caught:
                main.start_recording(main.StartRequest(name="../outside"))
            self.assertEqual(caught.exception.status_code, 400)

    def test_valid_names_and_literal_brackets(self):
        for name in ("Team meeting", "Reunión 2026", "Plan [draft]", "meeting.mp3", "meeting.wav"):
            self.assertTrue(MeetingRecorder._validate_stem(name))
        with tempfile.TemporaryDirectory() as temp, patch("app.recorder.RECORDINGS_DIR", Path(temp)), \
                patch.object(db, "get_transcription", return_value=None):
            (Path(temp) / "Plan [draft].wav").touch()
            self.assertEqual(MeetingRecorder._unique_stem("Plan [draft]"), "Plan [draft]_1")
