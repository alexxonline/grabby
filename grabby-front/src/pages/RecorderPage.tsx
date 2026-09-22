import { useState, useEffect, useRef } from "preact/hooks";
import { api } from "../api";
import type { RecordingStatus } from "../api";
import { RecordingTabs } from "../components/RecordingTabs";

export function RecorderPage({ path: _path }: { path?: string }) {
  const [status, setStatus] = useState<RecordingStatus>({
    recording: false,
    filename: null,
    started_at: null,
  });
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [operation, setOperation] = useState<"starting" | "stopping" | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const timerRef = useRef<number | null>(null);
  const [savedFile, setSavedFile] = useState<string | null>(null);
  const [liveError, setLiveError] = useState(false);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const followLive = useRef(true);
  useEffect(() => {
    if (followLive.current && transcriptRef.current) {
      transcriptRef.current.scrollTop = transcriptRef.current.scrollHeight;
    }
  }, [status.live]);

  // Poll status on mount
  useEffect(() => {
    let active = true;
    let timeout: number;
    const poll = async () => {
      try {
        const result = await api.getStatus();
        if (active) { setStatus(result); setLiveError(false); }
      } catch { if (active) setLiveError(true); }
      if (active) timeout = window.setTimeout(poll, 500);
    };
    void poll();
    return () => { active = false; window.clearTimeout(timeout); };
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
    setOperation("starting");
    try {
      const res = await api.startRecording(name || undefined);
      setStatus(res);
      setName("");
      setSavedFile(null);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setOperation(null);
    }
  };

  const handleStop = async () => {
    setError(null);
    setOperation("stopping");
    try {
      const res = await api.stopRecording();
      setStatus(res);
      setSavedFile(res.filename);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setOperation(null);
    }
  };

  return (
    <div class="page">
      <h1>Meeting Recorder</h1>

      <div class="recorder-card">
        {/* Status indicator */}
        <div class={`status-badge ${status.recording ? "recording" : "idle"}`}>
          <span class="status-dot" />
          {operation === "stopping" || status.saving ? "Saving recording…" : operation === "starting" ? "Starting…" : status.recording ? "Recording" : status.save_pending ? "Save required" : "Ready"}
        </div>

        {/* Timer */}
        {status.recording && (
          <div class="timer">{formatTime(elapsed)}</div>
        )}

        {/* Name input */}
        {!status.recording && !status.save_pending && operation === null && (
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
          <button
            class={`btn ${operation === "stopping" || (operation === null && status.recording) ? "btn-stop" : "btn-start"}`}
            onClick={status.recording || status.save_pending ? handleStop : handleStart}
            disabled={operation !== null || status.saving}
          >
            {operation === "stopping" || status.saving ? "Saving…" : operation === "starting" ? "Starting…" : status.recording ? "Stop Recording" : status.save_pending ? "Retry Save" : "Start Recording"}
          </button>
        </div>

        {(error || status.error) && <p class="error" role="alert">{error || status.error}</p>}
        {liveError && <p class="error">Cannot reach the recorder. Reconnecting…</p>}
        {savedFile && !status.recording && (
          <p>Recording saved. The final transcript with speaker diarization is processing.
            {" "}<a href="/recordings">View recordings</a>
          </p>
        )}
      </div>
      {status.notes_id && (
        <RecordingTabs notesId={status.notes_id} live={status.recording}>
        {status.recording ? (
        <section class="live-transcript">
          <h2>Live transcript</h2>
          <p>Preview only. The saved transcript is generated with speaker diarization after recording.</p>
          <p>You = microphone · Others = system audio</p>
          {(["you", "others"] as const).map(source => (
            <p key={source} class={status.live?.sources[source].error ? "error" : ""}>
              <strong>{source === "you" ? "You" : "Others"}:</strong>{" "}
              {status.live?.sources[source].error || status.live?.sources[source].status || "connecting"}
            </p>
          ))}
          <div class="live-transcript-text" ref={transcriptRef} role="log" aria-label="Live transcript"
            onScroll={e => {
              const el = e.currentTarget;
              followLive.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
            }}>
            {!status.live?.segments.length && !status.live?.sources.you.partial && !status.live?.sources.others.partial && (
              <p>Waiting for speech…</p>
            )}
            {status.live?.segments.map((segment, index) => (
              <p key={index}><strong>{segment.source === "you" ? "You" : "Others"}:</strong> {segment.text}</p>
            ))}
            {(["you", "others"] as const).map(source => status.live?.sources[source].partial && (
              <p key={source} class="live-partial"><strong>{source === "you" ? "You" : "Others"}:</strong> {status.live.sources[source].partial}</p>
            ))}
          </div>
        </section>
        ) : <p class="empty">Recording finished. <a href="/recordings">View the final transcript in Recordings.</a></p>}
        </RecordingTabs>
      )}
    </div>
  );
}
