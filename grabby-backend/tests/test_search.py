import sqlite3
import unittest
from unittest.mock import patch

from app import database as db


class SearchTests(unittest.TestCase):
    def setUp(self):
        self.conn = sqlite3.connect(":memory:")
        self.conn.row_factory = sqlite3.Row
        self.connection = patch.object(db, "_get_conn", return_value=self.conn)
        self.connection.start()
        db.init_db()
        db.save_transcription("meeting.wav", 'Speaker 1: Follow-up with José about the launch. Say "hello". Research and development.')
        db.save_transcription("other.wav", "Speaker 1: Follow a different launch plan.")

    def tearDown(self):
        self.connection.stop()
        self.conn.close()

    def test_punctuation_and_unicode_are_literal_search_terms(self):
        for query in ("follow-up", '"hello"', "José", "research AND development"):
            with self.subTest(query=query):
                self.assertEqual([r["filename"] for r in db.search_transcriptions(query)], ["meeting.wav"])

    def test_empty_or_punctuation_only_queries_do_not_raise(self):
        for query in ("", "   ", '"', "*", "(", ":", "-", '""'):
            with self.subTest(query=query):
                self.assertEqual(db.search_transcriptions(query), [])

    def test_multiple_terms_still_require_all_terms_and_highlight(self):
        results = db.search_transcriptions("José launch")
        self.assertEqual(len(results), 1)
        self.assertIn("**José**", results[0]["snippet"])
        self.assertIn("**launch**", results[0]["snippet"])
        self.assertEqual(db.search_transcriptions("José missing"), [])
