import { useState, useEffect, useRef } from "preact/hooks";
import { route } from "preact-router";
import { api } from "../api";
import type { RecordingInfo, TranscriptionStatus } from "../api";
import { AudioPlayer } from "../components/AudioPlayer";

export function RecordingsPage({ path: _path }: { path?: string }) {
  const [recordings, setRecordings] = useState<RecordingInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [editingFile, setEditingFile] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [playingRec, setPlayingRec] = useState<RecordingInfo | null>(null);

  // Transcription state
  const [transcriptions, setTranscriptions] = useState<Record<string, TranscriptionStatus>>({});
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

  useEffect(() => {
    if (!recordings.some(rec => rec.processing)) return;
    const timer = window.setInterval(() => {
      api.listRecordings().then(data => {
        setRecordings(data);
        setPlayingRec(current => current && data.find(rec => rec.filename === current.filename && rec.audio_available) || null);
      }).catch(e => setError(e.message));
    }, 2000);
    return () => clearInterval(timer);
  }, [recordings]);

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

  useEffect(() => {
    Object.entries(transcriptions).forEach(([filename, status]) => {
      if (status.status === "pending" && !pollTimers.current[filename]) {
        // Poll for completion
        const timer = window.setInterval(async () => {
          try {
            const updated = await api.getTranscription(filename);
            setTranscriptions((prev) => ({ ...prev, [filename]: updated }));
            if (updated.status === "completed" || updated.status === "error") {
              clearInterval(timer);
              delete pollTimers.current[filename];
            }
          } catch {
            // Retry transient network failures on the next interval.
          }
        }, 2000);
        pollTimers.current[filename] = timer;
      }
    });
  }, [transcriptions]);

  const handleTranscribe = async (rec: RecordingInfo) => {
    try {
      const status = await api.transcribeRecording(rec.filename);
      setTranscriptions((prev) => ({ ...prev, [rec.filename]: status }));
      await fetchRecordings();

      if (status.status === "completed") {
        route(`/transcription/${encodeURIComponent(rec.filename)}`);
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleDelete = async (filename: string) => {
    if (!confirm(`Delete "${filename}"?`)) return;
    try {
      await api.deleteRecording(filename);
      setPlayingRec(current => current?.filename === filename ? null : current);
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
                  <span>{rec.audio_available ? formatSize(rec.size_bytes) : "Transcript only"}</span>
                </div>
              </div>
              <div class="recording-actions">
                {rec.audio_available && <button
                  class={`btn btn-sm btn-play${playingRec?.filename === rec.filename ? " active" : ""}`}
                  onClick={() => setPlayingRec(playingRec?.filename === rec.filename ? null : rec)}
                  title={playingRec?.filename === rec.filename ? "Stop" : "Play"}
                >
                  {playingRec?.filename === rec.filename ? "⏹" : "▶"}
                </button>}
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
                        class="btn btn-sm btn-transcribe done"
                        onClick={() => route(`/transcription/${encodeURIComponent(rec.filename)}`)}
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
                {rec.audio_available && <a
                  class="btn btn-sm btn-download"
                  href={api.downloadUrl(rec.filename)}
                  download
                >
                  Download
                </a>}
                <button
                  class="btn btn-sm btn-rename"
                  onClick={() => handleRenameStart(rec)}
                  disabled={editingFile === rec.filename || rec.processing}
                >
                  Rename
                </button>
                <button
                  class="btn btn-sm btn-delete"
                  onClick={() => handleDelete(rec.filename)}
                  disabled={rec.processing}
                >
                  Delete
                </button>
              </div>
              </div>{/* end recording-row */}
              {rec.processing && <p class="recording-meta">Processing recording…</p>}
              {rec.processing_error && <p class="error">{rec.processing_error}</p>}
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
