import { useState, useEffect, useRef } from "preact/hooks";
import { route } from "preact-router";
import { api } from "../api";
import type { TranscriptionStatus } from "../api";
import { RecordingTabs } from "../components/RecordingTabs";

import { extractSpeakers, replaceSpeaker } from "../components/speakers";
import type { SpeakerEntry } from "../components/speakers";

export function TranscriptionPage({
  path: _path,
  filename,
}: {
  path?: string;
  filename?: string;
}) {
  const [transcription, setTranscription] = useState<TranscriptionStatus | null>(null);
  const [displayText, setDisplayText] = useState("");
  const [speakers, setSpeakers] = useState<SpeakerEntry[]>([]);
  const [editingIdx, setEditingIdx] = useState<number | null>(null);
  const [editValue, setEditValue] = useState("");
  const [saving, setSaving] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const renameInFlight = useRef(false);
  const loadedText = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!filename) return;
    let active = true;
    let timer: number;
    loadedText.current = false;
    setLoading(true);
    setError(null);
    setDisplayText("");
    setSpeakers([]);
    const poll = () => api.getTranscription(filename).then((ts) => {
        if (!active) return;
        setTranscription(ts);
        if (ts.status === "completed" && ts.text != null && !loadedText.current) {
          loadedText.current = true;
          setDisplayText(ts.text);
          setSpeakers(extractSpeakers(ts.text));
        }
        if (ts.status === "pending" || ts.processing) timer = window.setTimeout(poll, 2000);
      })
      .catch((e: Error) => { if (active) setError(e.message); })
      .finally(() => { if (active) setLoading(false); });
    void poll();
    return () => { active = false; clearTimeout(timer); };
  }, [filename]);

  useEffect(() => {
    if (editingIdx !== null && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingIdx]);

  const startEditing = (idx: number) => {
    setEditingIdx(idx);
    setEditValue(speakers[idx].current);
  };

  const cancelEditing = () => {
    setEditingIdx(null);
    setEditValue("");
  };

  const confirmRename = async (idx: number) => {
    if (renameInFlight.current) return;
    const newName = editValue.trim();
    if (!newName || !filename) {
      cancelEditing();
      return;
    }

    if (/[:\r\n]/.test(newName)) {
      setError("Speaker names cannot contain colons or line breaks.");
      return;
    }
    if (speakers.some((speaker, index) => index !== idx && speaker.current === newName)) {
      setError("Choose a different name for each speaker.");
      return;
    }
    setError(null);

    const speaker = speakers[idx];
    if (newName === speaker.current) {
      cancelEditing();
      return;
    }

    // Prepare the edit, then publish it only after the server accepts it.
    const updatedText = replaceSpeaker(displayText, speaker.current, newName);
    const updatedSpeakers = speakers.map((s, i) =>
      i === idx ? { ...s, current: newName } : s
    );

    // Persist to backend
    renameInFlight.current = true;
    setSaving(true);
    try {
      await api.updateTranscription(filename, updatedText);
      setDisplayText(updatedText);
      setSpeakers(updatedSpeakers);
      setEditingIdx(null);
      setEditValue("");
    } catch (e: any) {
      setError(e.message);
    } finally {
      renameInFlight.current = false;
      setSaving(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent, idx: number) => {
    if (e.key === "Enter") {
      e.preventDefault();
      confirmRename(idx);
    } else if (e.key === "Escape") {
      cancelEditing();
    }
  };

  /** Render transcript text with speaker labels highlighted. */
  function renderTranscript(text: string) {
    // Split into lines and highlight speaker labels
    const lines = text.split("\n");
    return lines.map((line, i) => {
      const match = line.match(/^(.+?):(.*)/);
      if (match) {
        return (
          <div class="transcript-line" key={i}>
            <span class="speaker-label">{match[1]}:</span>
            <span>{match[2]}</span>
          </div>
        );
      }
      return (
        <div class="transcript-line" key={i}>
          {line || "\u00A0"}
        </div>
      );
    });
  }

  if (loading) {
    return (
      <div class="page">
        <p class="loading">Loading transcription…</p>
      </div>
    );
  }

  if (error && !filename) {
    return (
      <div class="page">
        <p class="error">{error}</p>
        <button class="btn btn-sm btn-cancel" onClick={() => history.back()}>
          ← Back
        </button>
      </div>
    );
  }

  if (!filename) {
    return (
      <div class="page">
        <p class="empty">No transcription available for this recording.</p>
        <button class="btn btn-sm btn-cancel" onClick={() => history.back()}>
          ← Back
        </button>
      </div>
    );
  }

  const decodedName = filename ? filename.replace(/\.[^.]+$/, "") : "";

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(displayText);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("Failed to copy to clipboard");
    }
  };

  return (
    <div class="page transcription-page">
      <div class="transcription-top-bar">
        <button class="btn btn-sm btn-cancel" onClick={() => route("/recordings")}>
          ← Back
        </button>
        <h1>{transcription?.title || decodedName}</h1>
        <button class="btn btn-sm btn-copy" disabled={!displayText} onClick={copyToClipboard}>
          {copied ? "✓ Copied" : "📋 Copy"}
        </button>
      </div>

      <RecordingTabs filename={filename} initialNotes={new URLSearchParams(location.search).get("tab") === "notes"}>
      {error && <p class="error">{error}</p>}
      {transcription?.processing && <p role="status">Finishing recording title and cleanup...</p>}
      {transcription?.status === "completed" && transcription.error && <p class="error">{transcription.error}</p>}
      {transcription?.status !== "completed" && <p class="empty">
        {transcription?.status === "pending" ? "The transcript is processing. You can add notes now." :
          transcription?.status === "error" ? `Transcription failed: ${transcription.error}` : "No transcription available yet. You can still add notes."}
      </p>}
      {/* Speaker name editor */}
      {speakers.length > 0 && (
        <div class="speaker-editor">
          <div class="speaker-editor-label">Speakers — click a name to rename</div>
          <div class="speaker-chips">
            {speakers.map((s, idx) => (
              <div class="speaker-chip" key={s.original}>
                {editingIdx === idx ? (
                  <input
                    ref={inputRef}
                    disabled={saving}
                    class="speaker-chip-input"
                    type="text"
                    value={editValue}
                    onInput={(e) =>
                      setEditValue((e.target as HTMLInputElement).value)
                    }
                    onKeyDown={(e) => handleKeyDown(e, idx)}
                    onBlur={() => confirmRename(idx)}
                  />
                ) : (
                  <button
                    class="speaker-chip-btn"
                    disabled={saving}
                    onClick={() => startEditing(idx)}
                    title="Click to rename"
                  >
                    {s.current}
                  </button>
                )}
              </div>
            ))}
          </div>
          {saving && <div class="speaker-saving">Saving…</div>}
        </div>
      )}

      {/* Transcript body */}
      {transcription?.status === "completed" && <div class="transcript-body">{renderTranscript(displayText)}</div>}
      </RecordingTabs>
    </div>
  );
}
