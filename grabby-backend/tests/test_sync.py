import json
import os
import sqlite3
import tempfile
import threading
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch
from app import database as db, sync

class SyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.patches = [patch.object(db, 'DB_PATH', self.root/'client.db'),
                        patch.object(db, '_local', threading.local()),
                        patch.object(sync, 'ENV_PATH', self.root/'.env'),
                        patch.dict(os.environ, {'GRABBY_SYNC_API_KEY':'test-secret'})]
        for p in self.patches: p.start()
        db.init_db()
        self.client = db._get_conn()
        self.server = sqlite3.connect(self.root/'server.db')
        self.server.row_factory = sqlite3.Row
        with patch.object(db, "_get_conn", return_value=self.server):
            db.init_db()
        self.transport = patch.object(sync, 'exchange', side_effect=self.send)
        self.transport.start()
        sync.save_settings(True, '', True)

    def tearDown(self):
        self.transport.stop()
        db.close_connection(); self.server.close()
        for p in reversed(self.patches): p.stop()
        self.temp.cleanup()

    def send(self, body, config):
        with patch.object(db, '_get_conn', return_value=self.server):
            return sync.receive(body)

    def recording(self, filename='meeting.mp3', text='Initial text'):
        db.ensure_metadata(filename, '2026-10-05T10:00:00')
        db.save_transcription(filename, text, 'en')
        note = db.create_notes(filename)
        db.save_notes(note['id'], 'Remember this')
        return dict(self.client.execute('SELECT * FROM sync_records WHERE filename=?',(filename,)).fetchone())

    def other_edit(self, record, text='Other device edit', deleted=False):
        remote = json.loads(self.server.execute('SELECT snapshot FROM sync_mock_records WHERE recording_id=?',(record['recording_id'],)).fetchone()[0])
        remote['base_revision'] = remote.pop('revision')
        remote['mutation_id'] = str(uuid.uuid4())
        remote['transcription']['text'] = text
        if deleted: remote['deleted_at'] = sync.now()
        self.send({'schema_version':1,'device_id':str(uuid.uuid4()),'cursor':0,'records':[remote]}, {})

    def test_upload_download_and_text_only(self):
        r = self.recording(); result = sync.run()
        self.assertEqual(result['uploaded'],1)
        stored = json.loads(self.server.execute('SELECT snapshot FROM sync_mock_records').fetchone()[0])
        self.assertEqual(stored['notes']['text'],'Remember this')
        self.assertNotIn('audio',stored)
        self.assertNotIn('test-secret',json.dumps(stored))
        self.other_edit(r)
        sync.run()
        self.assertEqual(db.get_transcription('meeting.mp3')['text'],'Other device edit')
        self.assertEqual(sync.run()['uploaded'],0)
        self.assertTrue(db.search_transcriptions('Other'))

    def test_conflict_keeps_both_and_local_resolution_rebases(self):
        r = self.recording(); sync.run(); self.other_edit(r)
        db.update_transcription_text('meeting.mp3','Local edit')
        sync.run()
        conflict = sync.conflicts()[0]
        self.assertEqual(conflict['local']['transcription']['text'],'Local edit')
        self.assertEqual(conflict['server']['transcription']['text'],'Other device edit')
        sync.resolve(r['recording_id'],'local'); sync.run()
        self.assertEqual(sync.conflicts(),[])
        stored = json.loads(self.server.execute('SELECT snapshot FROM sync_mock_records').fetchone()[0])
        self.assertEqual(stored['transcription']['text'],'Local edit')

    def test_server_resolution_and_deletion_preserve_audio(self):
        r=self.recording(); sync.run(); self.other_edit(r)
        db.update_transcription_text('meeting.mp3','Local edit'); sync.run()
        sync.resolve(r['recording_id'],'server')
        self.assertEqual(db.get_transcription('meeting.mp3')['text'],'Other device edit')
        audio=self.root/'meeting.mp3'; audio.write_bytes(b'audio')
        self.other_edit(r,deleted=True); sync.run()
        self.assertIsNone(db.get_transcription('meeting.mp3'))
        self.assertTrue(audio.exists())
        row=self.client.execute('SELECT * FROM sync_records').fetchone()
        self.assertIsNotNone(row['deleted_at'])

    def test_local_deletion_tombstone_survives_notes_cleanup(self):
        self.recording(); sync.run()
        db.delete_transcription('meeting.mp3'); db.delete_metadata('meeting.mp3')
        row=self.client.execute('SELECT * FROM sync_records').fetchone()
        self.assertIsNotNone(row['deleted_at'])
        sync.run()
        stored=json.loads(self.server.execute('SELECT snapshot FROM sync_mock_records').fetchone()[0])
        self.assertIsNotNone(stored['deleted_at'])

    def test_retry_after_lost_response_is_idempotent(self):
        self.recording()
        def lose(body,c):
            self.send(body,c); raise TimeoutError()
        with patch.object(sync,'exchange',side_effect=lose):
            with self.assertRaises(TimeoutError): sync.run()
        result=sync.run()
        self.assertEqual(result['uploaded'],1)
        self.assertEqual(self.server.execute('SELECT count(*) FROM sync_mock_mutations').fetchone()[0],1)
        self.assertEqual(sync.conflicts(),[])

    def test_edit_during_upload_remains_pending(self):
        self.recording()
        def edit(body,c):
            db.update_transcription_text('meeting.mp3','Newer edit')
            return self.send(body,c)
        with patch.object(sync,'exchange',side_effect=edit): sync.run()
        row=self.client.execute('SELECT * FROM sync_records').fetchone()
        self.assertGreater(row['generation'],row['synced_generation'])
        self.assertEqual(db.get_transcription('meeting.mp3')['text'],'Newer edit')
        sync.run()
        stored=json.loads(self.server.execute('SELECT snapshot FROM sync_mock_records').fetchone()[0])
        self.assertEqual(stored['transcription']['text'],'Newer edit')

    def test_filename_collision_does_not_overwrite_local(self):
        local=self.recording(); sync.run()
        remote=sync.snapshot(self.client.execute('SELECT * FROM sync_records').fetchone())
        remote['recording_id']=str(uuid.uuid4()); remote['mutation_id']='new'; remote['base_revision']=0
        self.send({'schema_version':1,'device_id':str(uuid.uuid4()),'cursor':0,'records':[remote]}, {})
        sync.run()
        rows=list(self.client.execute('SELECT * FROM sync_records'))
        self.assertEqual(len(rows),2)
        self.assertEqual(len({r['filename'] for r in rows}),2)
        self.assertEqual(db.get_transcription(local['filename'])['text'],'Initial text')

    def test_settings_credentials_and_https_validation(self):
        for endpoint in ('http://example.com/sync','https://user:pass@example.com/sync','https://example.com/sync?key=secret'):
            with self.assertRaises(ValueError): sync.save_settings(True,endpoint,False)
        sync.save_settings(True,'https://example.com/sync',False,'new-key')
        self.assertNotIn('new-key',json.dumps(sync.settings()))
        self.assertIn('new-key',sync.ENV_PATH.read_text())
        self.assertNotIn('new-key',str(list(self.client.execute('SELECT * FROM sync_config'))))

    def test_mock_scenarios_failure_recovery_and_conflict(self):
        self.recording()
        # Use the actual internal mock transport and persistent receiver.
        self.transport.stop()
        sync.mock_scenario('remote_edit'); sync.run()
        self.assertIn('mock device edit', db.get_metadata('meeting.mp3')['title'])
        sync.mock_scenario('conflict'); sync.run()
        self.assertEqual(len(sync.conflicts()), 1)
        for scenario in ('failure','authentication'):
            sync.mock_scenario(scenario)
            with self.assertRaises(Exception): sync.run()
            self.assertIsNotNone(sync.settings()['error'])
            sync.run()
            self.assertIsNone(sync.settings()['error'])

    def test_receiving_endpoint_authentication(self):
        from fastapi import HTTPException
        from starlette.requests import Request
        from app import main
        body={'schema_version':1,'device_id':str(uuid.uuid4()),'cursor':0,'records':[]}
        def request(key, host='127.0.0.1'):
            return Request({'type':'http','client':(host,1234),'headers':[(b'authorization',key.encode())]})
        with self.assertRaises(HTTPException) as caught:
            main.mock_exchange(request('Bearer wrong'),body)
        self.assertEqual(caught.exception.status_code,401)
        with self.assertRaises(HTTPException) as caught:
            main.mock_exchange(request('Bearer test-secret', '10.0.0.2'),body)
        self.assertEqual(caught.exception.status_code,404)
        self.assertEqual(main.mock_exchange(request('Bearer test-secret'),body)['schema_version'],1)
        with self.assertRaises(HTTPException) as caught:
            main.mock_exchange(request('Bearer test-secret'),{})
        self.assertEqual(caught.exception.status_code,400)

    def test_bad_response_does_not_advance_cursor_or_acknowledge(self):
        self.recording()
        bad={'schema_version':1,'cursor':50,'acknowledgments':[{'recording_id':'wrong','mutation_id':'wrong','revision':1}], 'changes':[], 'conflicts':[]}
        with patch.object(sync,'exchange',return_value=bad):
            with self.assertRaises(ValueError): sync.run()
        row=self.client.execute('SELECT * FROM sync_records').fetchone()
        self.assertEqual(row['synced_generation'],0)
        self.assertEqual(int(sync.config().get('cursor',0)),0)

    def test_unattached_notes_wait_and_repeatable_migration(self):
        note=db.create_notes(); db.save_notes(note['id'],'Draft')
        self.assertEqual(sync.run()['uploaded'],0)
        db.attach_notes(note['id'],'later.mp3'); db.ensure_metadata('later.mp3',sync.now())
        self.assertEqual(sync.run()['uploaded'],1)
        device=sync.settings()['device_id']; db.init_db()
        self.assertEqual(sync.settings()['device_id'],device)

if __name__=='__main__': unittest.main()
