import { useState, useEffect, useRef } from "preact/hooks";
import { api } from "../api";
import type { RecordingInfo, TranscriptionStatus } from "../api";
import { AudioPlayer } from "../components/AudioPlayer";

export function RecordingsPage({ path }: { path?: string }) {
  const [recordings, setRecordings] = useState<RecordingInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [editingFile, setEditingFile] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [playingRec, setPlayingRec] = useState<RecordingInfo | null>(null);

  // Transcription state
  const [transcriptions, setTranscriptions] = useState<Record<string, TranscriptionStatus>>({});
  const [expandedTranscript, setExpandedTranscript] = useState<string | null>(null);
  const pollTimers = useRef<Record<string, number>>({});

  const fetchRecordings = async () => {
    setLoading(true);
    try {
      const data = await api.listRecordings();
      setRecordings(data);
      setError(null);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchRecordings();
  }, []);

  // Load transcription status for all recordings
  useEffect(() => {
    recordings.forEach((rec) => {
      if (!transcriptions[rec.filename]) {
        api.getTranscription(rec.filename).then((status) => {
          if (status.status !== "none") {
            setTranscriptions((prev) => ({ ...prev, [rec.filename]: status }));
          }
        }).catch(() => {});
      }
    });
  }, [recordings]);

  // Cleanup poll timers on unmount
  useEffect(() => {
    return () => {
      Object.values(pollTimers.current).forEach(clearInterval);
    };
  }, []);

  const handleTranscribe = async (rec: RecordingInfo) => {
    try {
      const status = await api.transcribeRecording(rec.filename);
      setTranscriptions((prev) => ({ ...prev, [rec.filename]: status }));

      if (status.status === "pending") {
        // Poll for completion
        const timer = window.setInterval(async () => {
          try {
            const updated = await api.getTranscription(rec.filename);
            setTranscriptions((prev) => ({ ...prev, [rec.filename]: updated }));
            if (updated.status === "completed" || updated.status === "error") {
              clearInterval(timer);
              delete pollTimers.current[rec.filename];
            }
          } catch {
            clearInterval(timer);
            delete pollTimers.current[rec.filename];
          }
        }, 2000);
        pollTimers.current[rec.filename] = timer;
      }

      if (status.status === "completed") {
        setExpandedTranscript(rec.filename);
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const toggleTranscript = (filename: string) => {
    setExpandedTranscript((prev) => (prev === filename ? null : filename));
  };

  const handleDelete = async (filename: string) => {
    if (!confirm(`Delete "${filename}"?`)) return;
    try {
      await api.deleteRecording(filename);
      setRecordings((prev) => prev.filter((r) => r.filename !== filename));
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleRenameStart = (rec: RecordingInfo) => {
    setEditingFile(rec.filename);
    setEditName(rec.name);
  };

  const handleRenameConfirm = async () => {
    if (!editingFile) return;
    try {
      const updated = await api.renameRecording(editingFile, editName);
      setRecordings((prev) =>
        prev.map((r) => (r.filename === editingFile ? updated : r))
      );
      setEditingFile(null);
      setEditName("");
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleRenameCancel = () => {
    setEditingFile(null);
    setEditName("");
  };

  const formatDate = (iso: string) => {
    const d = new Date(iso);
    return d.toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  };

  const formatSize = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  return (
    <div class="page">
      <h1>Recordings</h1>

      {error && <p class="error">{error}</p>}

      {loading ? (
        <p class="loading">Loading recordings…</p>
      ) : recordings.length === 0 ? (
        <p class="empty">No recordings yet. Start a recording first!</p>
      ) : (
        <div class="recordings-list">
          {recordings.map((rec) => (
            <div class="recording-item" key={rec.filename}>
              <div class="recording-row">
                <div class="recording-info">
                {editingFile === rec.filename ? (
                  <div class="rename-form">
                    <input
                      type="text"
                      value={editName}
                      onInput={(e) =>
                        setEditName((e.target as HTMLInputElement).value)
                      }
                      onKeyDown={(e) => {
                        if (e.key === "Enter") handleRenameConfirm();
                        if (e.key === "Escape") handleRenameCancel();
                      }}
                      autoFocus
                    />
                    <button class="btn btn-sm btn-save" onClick={handleRenameConfirm}>
                      Save
                    </button>
                    <button class="btn btn-sm btn-cancel" onClick={handleRenameCancel}>
                      Cancel
                    </button>
                  </div>
                ) : (
                  <div class="recording-name">{rec.name}</div>
                )}
                <div class="recording-meta">
                  <span>{formatDate(rec.created_at)}</span>
                  <span class="sep">•</span>
                  <span>{formatSize(rec.size_bytes)}</span>
                </div>
              </div>
              <div class="recording-actions">
                <button
                  class={`btn btn-sm btn-play${playingRec?.filename === rec.filename ? " active" : ""}`}
                  onClick={() => setPlayingRec(playingRec?.filename === rec.filename ? null : rec)}
                  title={playingRec?.filename === rec.filename ? "Stop" : "Play"}
                >
                  {playingRec?.filename === rec.filename ? "⏹" : "▶"}
                </button>
                {(() => {
                  const ts = transcriptions[rec.filename];
                  if (ts?.status === "pending") {
                    return (
                      <button class="btn btn-sm btn-transcribe" disabled title="Transcribing…">
                        <span class="spinner" /> Transcribing…
                      </button>
                    );
                  }
                  if (ts?.status === "completed") {
                    return (
                      <button
                        class={`btn btn-sm btn-transcribe done${expandedTranscript === rec.filename ? " active" : ""}`}
                        onClick={() => toggleTranscript(rec.filename)}
                        title="View transcript"
                      >
                        📝 Transcript
                      </button>
                    );
                  }
                  if (ts?.status === "error") {
                    return (
                      <button
                        class="btn btn-sm btn-transcribe error"
                        onClick={() => handleTranscribe(rec)}
                        title={`Error: ${ts.error}. Click to retry.`}
                      >
                        ⚠ Retry
                      </button>
                    );
                  }
                  return (
                    <button
                      class="btn btn-sm btn-transcribe"
                      onClick={() => handleTranscribe(rec)}
                      title="Transcribe recording"
                    >
                      Transcribe
                    </button>
                  );
                })()}
                <a
                  class="btn btn-sm btn-download"
                  href={api.downloadUrl(rec.filename)}
                  download
                >
                  Download
                </a>
                <button
                  class="btn btn-sm btn-rename"
                  onClick={() => handleRenameStart(rec)}
                  disabled={editingFile === rec.filename}
                >
                  Rename
                </button>
                <button
                  class="btn btn-sm btn-delete"
                  onClick={() => handleDelete(rec.filename)}
                >
                  Delete
                </button>
              </div>
              </div>{/* end recording-row */}

              {/* Transcript panel */}
              {expandedTranscript === rec.filename && transcriptions[rec.filename]?.text && (
                <div class="transcript-panel">
                  <div class="transcript-header">
                    <span class="transcript-label">Transcript</span>
                    <button
                      class="btn btn-sm btn-cancel"
                      onClick={() => setExpandedTranscript(null)}
                    >
                      Close
                    </button>
                  </div>
                  <pre class="transcript-text">{transcriptions[rec.filename].text}</pre>
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {playingRec && (
        <AudioPlayer
          src={api.downloadUrl(playingRec.filename)}
          title={playingRec.name}
          onClose={() => setPlayingRec(null)}
        />
      )}
    </div>
  );
}
