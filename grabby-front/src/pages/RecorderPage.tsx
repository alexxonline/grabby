import { useState, useEffect, useRef } from "preact/hooks";
import { api } from "../api";
import type { RecordingStatus } from "../api";

export function RecorderPage({ path }: { path?: string }) {
  const [status, setStatus] = useState<RecordingStatus>({
    recording: false,
    filename: null,
    started_at: null,
  });
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const timerRef = useRef<number | null>(null);

  // Poll status on mount
  useEffect(() => {
    api.getStatus().then(setStatus).catch(() => {});
  }, []);

  // Elapsed timer
  useEffect(() => {
    if (status.recording && status.started_at) {
      const start = new Date(status.started_at).getTime();
      const tick = () => {
        setElapsed(Math.floor((Date.now() - start) / 1000));
      };
      tick();
      timerRef.current = window.setInterval(tick, 1000);
      return () => {
        if (timerRef.current) clearInterval(timerRef.current);
      };
    } else {
      setElapsed(0);
      if (timerRef.current) clearInterval(timerRef.current);
    }
  }, [status.recording, status.started_at]);

  const formatTime = (s: number) => {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = s % 60;
    return [h, m, sec].map((v) => String(v).padStart(2, "0")).join(":");
  };

  const handleStart = async () => {
    setError(null);
    setLoading(true);
    try {
      const res = await api.startRecording(name || undefined);
      setStatus(res);
      setName("");
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleStop = async () => {
    setError(null);
    setLoading(true);
    try {
      const res = await api.stopRecording();
      setStatus(res);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div class="page">
      <h1>Meeting Recorder</h1>

      <div class="recorder-card">
        {/* Status indicator */}
        <div class={`status-badge ${status.recording ? "recording" : "idle"}`}>
          <span class="status-dot" />
          {status.recording ? "Recording" : "Ready"}
        </div>

        {/* Timer */}
        {status.recording && (
          <div class="timer">{formatTime(elapsed)}</div>
        )}

        {/* Name input */}
        {!status.recording && (
          <div class="name-input">
            <input
              type="text"
              placeholder="Recording name (optional)"
              value={name}
              onInput={(e) => setName((e.target as HTMLInputElement).value)}
            />
          </div>
        )}

        {/* Current file */}
        {status.recording && status.filename && (
          <p class="current-file">File: {status.filename}.mp3</p>
        )}

        {/* Controls */}
        <div class="controls">
          {!status.recording ? (
            <button
              class="btn btn-start"
              onClick={handleStart}
              disabled={loading}
            >
              {loading ? "Starting…" : "Start Recording"}
            </button>
          ) : (
            <button
              class="btn btn-stop"
              onClick={handleStop}
              disabled={loading}
            >
              {loading ? "Stopping…" : "Stop Recording"}
            </button>
          )}
        </div>

        {error && <p class="error">{error}</p>}
      </div>
    </div>
  );
}
