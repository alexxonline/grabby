import { useState, useEffect, useRef } from "preact/hooks";
import { route } from "preact-router";
import { api } from "../api";
import type { TranscriptionStatus } from "../api";

interface SpeakerEntry {
  original: string; // e.g. "Speaker A"
  current: string;  // edited name
}

/** Extract unique "Speaker X" labels from transcript text. */
function extractSpeakers(text: string): SpeakerEntry[] {
  const regex = /^(Speaker [A-Z0-9]+):/gm;
  const seen = new Set<string>();
  let match: RegExpExecArray | null;
  while ((match = regex.exec(text)) !== null) {
    seen.add(match[1]);
  }
  return Array.from(seen)
    .sort()
    .map((s) => ({ original: s, current: s }));
}

/** Replace all occurrences of a speaker label in the text. */
function replaceSpeaker(text: string, oldName: string, newName: string): string {
  // Replace "OldName:" at start of line with "NewName:"
  const escaped = oldName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return text.replace(new RegExp(`^${escaped}:`, "gm"), `${newName}:`);
}

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
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!filename) return;
    setLoading(true);
    api
      .getTranscription(filename)
      .then((ts) => {
        setTranscription(ts);
        if (ts.status === "completed" && ts.text) {
          setDisplayText(ts.text);
          setSpeakers(extractSpeakers(ts.text));
        }
      })
      .catch((e: any) => setError(e.message))
      .finally(() => setLoading(false));
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
    const newName = editValue.trim();
    if (!newName || !filename) {
      cancelEditing();
      return;
    }

    const speaker = speakers[idx];
    if (newName === speaker.current) {
      cancelEditing();
      return;
    }

    // Replace in displayed text
    const updatedText = replaceSpeaker(displayText, speaker.current, newName);
    setDisplayText(updatedText);

    // Update speakers list
    const updatedSpeakers = speakers.map((s, i) =>
      i === idx ? { ...s, current: newName } : s
    );
    setSpeakers(updatedSpeakers);
    setEditingIdx(null);
    setEditValue("");

    // Persist to backend
    setSaving(true);
    try {
      await api.updateTranscription(filename, updatedText);
    } catch (e: any) {
      setError(e.message);
    } finally {
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

  if (error) {
    return (
      <div class="page">
        <p class="error">{error}</p>
        <button class="btn btn-sm btn-cancel" onClick={() => history.back()}>
          ← Back
        </button>
      </div>
    );
  }

  if (!transcription || transcription.status !== "completed" || !displayText) {
    return (
      <div class="page">
        <p class="empty">No transcription available for this recording.</p>
        <button class="btn btn-sm btn-cancel" onClick={() => history.back()}>
          ← Back
        </button>
      </div>
    );
  }

  const decodedName = filename ? decodeURIComponent(filename).replace(/\.[^.]+$/, "") : "";

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
        <h1>Transcript: {decodedName}</h1>
        <button class="btn btn-sm btn-copy" onClick={copyToClipboard}>
          {copied ? "✓ Copied" : "📋 Copy"}
        </button>
      </div>

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
      <div class="transcript-body">{renderTranscript(displayText)}</div>
    </div>
  );
}
