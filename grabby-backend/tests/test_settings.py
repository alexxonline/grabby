"""Regression tests use temporary audio/database files and mocked providers."""
import io
import json
import os
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch
from fastapi import BackgroundTasks, HTTPException
from app import database as db

# Import the app without touching the real database.
with patch.object(db, 'init_db'):
    from app import main
from app import titles
from app.recorder import MeetingRecorder


class SettingsProcessingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.patches = [patch.object(db, 'DB_PATH', self.root / 'test.db'),
                        patch.object(db, '_local', threading.local()),
                        patch.object(main, 'RECORDINGS_DIR', self.root),
                        patch('app.recorder.RECORDINGS_DIR', self.root)]
        for item in self.patches:
            item.start()
        db.init_db()
        main._transcribing.clear()
        main._transcribe_errors.clear()
        self.audio = self.root / 'meeting.wav'
        self.audio.write_bytes(b'audio fixture')

    def tearDown(self):
        db._get_conn().close()
        for item in reversed(self.patches):
            item.stop()
        self.temp.cleanup()

    def process(self, **settings):
        db.save_settings(settings)
        main._transcribing[self.audio.name] = 'pending'
        with patch.object(main, 'transcribe_file', return_value={'text': 'Speaker 1: Project launch', 'language': 'en'}):
            main._do_transcribe(self.audio.name)

    def test_defaults_and_settings_persist_without_exposing_key(self):
        self.assertEqual(db.get_settings(), {'auto_delete_audio': False, 'auto_title': False})
        with patch.dict(os.environ, {'OPENROUTER_API_KEY': 'secret-test-value'}):
            result = main.update_settings(main.SettingsUpdate(auto_delete_audio=True, auto_title=True))
        self.assertNotIn('secret-test-value', str(result))
        db._get_conn().close()
        db._local.conn = None
        self.assertTrue(db.get_settings()['auto_delete_audio'])
        db.init_db()  # Migration is repeatable.
        self.assertTrue(db.get_settings()['auto_title'])

    def test_delete_audio_preserves_list_search_rename_and_transcript(self):
        before = main.list_recordings()[0].created_at
        self.process(auto_delete_audio=True)
        self.assertFalse(self.audio.exists())
        rec = main.list_recordings()[0]
        self.assertFalse(rec.audio_available)
        self.assertEqual(rec.created_at, before)
        self.assertEqual(main.get_transcription(self.audio.name).status, 'completed')
        self.assertEqual(main.start_transcription(self.audio.name, BackgroundTasks()).status, 'completed')
        main.rename_recording(self.audio.name, main.RenameRequest(new_name='Launch plan'))
        self.assertEqual(main.list_recordings()[0].name, 'Launch plan')
        self.assertEqual(main.search_transcriptions('Project')[0].title, 'Launch plan')
        self.assertEqual(MeetingRecorder._unique_stem('meeting'), 'meeting_1')
        main.delete_recording(self.audio.name)
        self.assertEqual(main.list_recordings(), [])
        self.assertEqual(db.search_transcriptions('Project'), [])

    def test_disabled_options_keep_audio_and_do_not_call_title_provider(self):
        with patch.object(main, 'suggest_title') as suggest:
            self.process()
        suggest.assert_not_called()
        self.assertTrue(self.audio.exists())

    def test_failed_transcription_or_database_save_keeps_audio(self):
        db.save_settings({'auto_delete_audio': True})
        with patch.object(main, 'transcribe_file', side_effect=RuntimeError('provider failed')):
            main._do_transcribe(self.audio.name)
        self.assertTrue(self.audio.exists())
        self.assertEqual(main.get_transcription(self.audio.name).status, 'error')
        with patch.object(db, 'save_transcription', side_effect=RuntimeError('disk full')):
            self.process(auto_delete_audio=True)
        self.assertTrue(self.audio.exists())
        self.assertIsNone(db.get_transcription(self.audio.name))

    def test_title_success_and_failure_are_independent_of_audio_cleanup(self):
        with patch.object(main, 'suggest_title', return_value='Project launch') as suggest:
            self.process(auto_title=True)
        self.assertEqual(main.list_recordings()[0].name, 'Project launch')
        suggest.assert_called_once_with('Speaker 1: Project launch')
        with patch.object(main, 'suggest_title', side_effect=RuntimeError('No credits')):
            self.process(auto_delete_audio=True, auto_title=True)
        rec = main.list_recordings()[0]
        self.assertEqual(rec.processing_error, 'No credits')
        self.assertEqual(rec.name, 'Project launch')
        self.assertFalse(rec.audio_available)
        self.assertEqual(main.get_transcription(self.audio.name).status, 'completed')

    def test_pending_recording_cannot_be_deleted_or_renamed(self):
        main._transcribing[self.audio.name] = 'pending'
        for action in (lambda: main.delete_recording(self.audio.name),
                       lambda: main.rename_recording(self.audio.name, main.RenameRequest(new_name='Changed'))):
            with self.assertRaises(HTTPException) as error:
                action()
            self.assertEqual(error.exception.status_code, 409)
        self.assertTrue(self.audio.exists())

    def test_audio_deletion_failure_retains_transcript_and_shows_error(self):
        with patch.object(Path, 'unlink', side_effect=PermissionError()):
            self.process(auto_delete_audio=True)
        self.assertTrue(self.audio.exists())
        self.assertIn('could not be deleted', main.list_recordings()[0].processing_error)
        self.assertEqual(main.get_transcription(self.audio.name).status, 'completed')


class TitleProviderTests(unittest.TestCase):
    def test_openrouter_request_uses_requested_model_and_bounds_input(self):
        response = io.BytesIO(json.dumps({'choices': [{'message': {'content': '"Launch plan"'}}]}).encode())
        with patch.dict(os.environ, {'OPENROUTER_API_KEY': 'test-key'}), patch.object(titles, 'urlopen', return_value=response) as send:
            self.assertEqual(titles.suggest_title('x' * 30000), 'Launch plan')
        request = send.call_args.args[0]
        body = json.loads(request.data)
        self.assertEqual(body['model'], 'openai/gpt-5.6-luna')
        self.assertEqual(len(body['messages'][1]['content']), 24000)
        self.assertEqual(request.get_header('Authorization'), 'Bearer test-key')

    def test_missing_key_and_empty_response(self):
        with patch.dict(os.environ, {'OPENROUTER_API_KEY': ''}):
            with self.assertRaisesRegex(RuntimeError, 'OPENROUTER_API_KEY'):
                titles.suggest_title('hello')
        with patch.dict(os.environ, {'OPENROUTER_API_KEY': 'test'}), patch.object(titles, 'urlopen', return_value=io.BytesIO(b'{"choices": []}')):
            with self.assertRaisesRegex(RuntimeError, 'no usable title'):
                titles.suggest_title('hello')


if __name__ == '__main__':
    unittest.main()
