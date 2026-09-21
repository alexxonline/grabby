"""Ephemeral source-labelled captions; never persisted as the final transcript."""

import asyncio
import audioop
import base64
import threading
from collections import deque
from queue import Empty, Full, Queue

from elevenlabs.realtime import AudioFormat, CommitStrategy, RealtimeEvents
from .transcription import get_client


class LiveTranscript:
    def __init__(self):
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._queues = {source: Queue(maxsize=250) for source in ("you", "others")}
        self._segments = deque(maxlen=500)
        self._sources = {source: {"status": "connecting", "partial": "", "error": None}
                         for source in self._queues}
        self._thread = threading.Thread(target=lambda: asyncio.run(self._run()), daemon=True)

    def start(self):
        self._thread.start()

    def stop(self):
        self._stop.set()

    def snapshot(self):
        with self._lock:
            return {"segments": list(self._segments),
                    "sources": {k: dict(v) for k, v in self._sources.items()}}

    def _update(self, source, **values):
        with self._lock:
            self._sources[source].update(values)

    def feed(self, source, data, rate, channels):
        if self._stop.is_set() or self._sources[source]["status"] == "error":
            return
        try:
            self._queues[source].put_nowait((data, rate, channels))
        except Full:
            self._update(source, status="error", error="Live audio fell behind. Restart recording to reconnect.")

    def _transcript(self, source, data, committed=False):
        text = data.get("text", "")
        with self._lock:
            if committed:
                if text.strip():
                    self._segments.append({"source": source, "text": text})
                self._sources[source]["partial"] = ""
            else:
                self._sources[source]["partial"] = text

    async def _run(self):
        await asyncio.gather(*(self._stream(source) for source in self._queues))

    async def _stream(self, source):
        connection = None
        try:
            connection = await asyncio.wait_for(get_client().speech_to_text.realtime.connect({
                "model_id": "scribe_v2_realtime", "audio_format": AudioFormat.PCM_16000,
                "sample_rate": 16000, "commit_strategy": CommitStrategy.VAD,
            }), timeout=10)
            connection.on(RealtimeEvents.PARTIAL_TRANSCRIPT, lambda data: self._transcript(source, data))
            connection.on(RealtimeEvents.COMMITTED_TRANSCRIPT, lambda data: self._transcript(source, data, True))
            connection.on(RealtimeEvents.ERROR, lambda data: self._update(
                source, status="error", error="Live transcription unavailable. Check ElevenLabs connection, quota and API key."))
            connection.on(RealtimeEvents.CLOSE, lambda: self._update(
                source, status="stopped" if self._stop.is_set() else "error",
                error=None if self._stop.is_set() else "Live connection closed. Restart recording to reconnect."))
            self._update(source, status="listening")
            state = None
            buffer = bytearray()
            while not self._stop.is_set():
                if self._sources[source]["status"] == "error":
                    break
                try:
                    data, rate, channels = self._queues[source].get_nowait()
                except Empty:
                    await asyncio.sleep(0.02)
                    continue
                if channels == 2:
                    data = audioop.tomono(data, 2, 0.5, 0.5)
                elif channels != 1:
                    # Average every channel for multichannel loopback devices.
                    from array import array
                    samples = array("h", data)
                    data = array("h", (sum(samples[i:i + channels]) // channels
                                       for i in range(0, len(samples), channels))).tobytes()
                data, state = audioop.ratecv(data, 2, 1, rate, 16000, state)
                buffer.extend(data)
                if len(buffer) >= 6400:  # At least 200 ms of mono PCM16.
                    await asyncio.wait_for(connection.send({
                        "audio_base_64": base64.b64encode(buffer).decode("ascii")}), timeout=3)
                    buffer.clear()
            # Preview is discarded on stop; the full saved audio is transcribed separately.
        except Exception:
            self._update(source, status="error", error="Live transcription unavailable. Check ElevenLabs connection, quota and API key.")
        finally:
            if connection:
                try:
                    await asyncio.wait_for(connection.close(), timeout=2)
                except Exception:
                    pass
            if self._stop.is_set():
                self._update(source, status="stopped", partial="")
