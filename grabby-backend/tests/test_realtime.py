import asyncio
import base64
import unittest
from array import array
from unittest.mock import patch, MagicMock

from app.realtime import LiveTranscript


class LiveTranscriptTests(unittest.TestCase):
    def test_partials_replace_and_sources_stay_separate(self):
        live = LiveTranscript()
        live._transcript("you", {"text": "hel"})
        live._transcript("you", {"text": "hello"})
        live._transcript("others", {"text": "hi"}, True)
        live._transcript("you", {"text": "hello"}, True)
        snapshot = live.snapshot()
        self.assertEqual(snapshot["sources"]["you"]["partial"], "")
        self.assertEqual(snapshot["segments"], [
            {"source": "others", "text": "hi"}, {"source": "you", "text": "hello"}])
        snapshot["sources"]["you"]["status"] = "mutated"
        self.assertEqual(live.snapshot()["sources"]["you"]["status"], "connecting")

    def test_queue_is_bounded_and_failure_does_not_raise(self):
        live = LiveTranscript()
        for _ in range(300):
            live.feed("you", b"00", 16000, 1)
        self.assertEqual(live._queues["you"].qsize(), 250)
        self.assertEqual(live.snapshot()["sources"]["you"]["status"], "error")
        live.stop()
        live.feed("others", b"00", 16000, 1)
        self.assertTrue(live._queues["others"].empty())

    def test_stream_downmixes_resamples_and_closes(self):
        live = LiveTranscript()
        sent = []
        closed = []

        class Connection:
            def on(self, event, callback):
                pass

            async def send(self, data):
                sent.append(base64.b64decode(data["audio_base_64"]))
                live.stop()

            async def close(self):
                closed.append(True)

        async def connect(options):
            self.assertEqual(options["model_id"], "scribe_v2_realtime")
            self.assertEqual(options["sample_rate"], 16000)
            return Connection()

        client = MagicMock()
        client.speech_to_text.realtime.connect = connect
        live.feed("you", array("h", [1000, 3000] * 12000).tobytes(), 48000, 2)
        with patch("app.realtime.get_client", return_value=client):
            asyncio.run(live._stream("you"))
        self.assertEqual(len(sent), 1)
        self.assertTrue(7900 <= len(sent[0]) <= 8100)
        self.assertEqual(set(array("h", sent[0])), {2000})
        self.assertEqual(closed, [True])

    def test_missing_key_does_not_fail_recording(self):
        live = LiveTranscript()
        with patch("app.realtime.get_client", side_effect=RuntimeError("missing key")):
            asyncio.run(live._run())
        self.assertTrue(all(s["status"] == "error" for s in live.snapshot()["sources"].values()))


if __name__ == "__main__":
    unittest.main()
