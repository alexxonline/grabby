import { useEffect, useState } from "preact/hooks";
import { api } from "../api";

// Serialize saves across tab/page changes so older requests cannot overwrite newer edits.
const pending = new Map<string, Promise<unknown>>();
const draftKey = (id: string) => `grabby:notes:${id}`;
function readDraft(id: string) {
  try { return localStorage.getItem(draftKey(id)); } catch { return null; }
}
function storeDraft(id: string, text: string) {
  try { localStorage.setItem(draftKey(id), text); } catch { /* Saving to the server still works. */ }
}

export function NotesEditor({ notesId, filename }: { notesId?: string; filename?: string }) {
  const [text, setText] = useState("");
  const [ready, setReady] = useState(false);
  const [status, setStatus] = useState("Loading notes…");
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const [edit, setEdit] = useState<{ change: (text: string) => void; save: () => void } | null>(null);

  useEffect(() => {
    let active = true;
    let id: string | undefined;
    let current = "";
    let saved = "";
    let timer: number | undefined;
    let inFlight: Promise<unknown> | undefined;
    setReady(false);
    setError("");
    setStatus("Loading notes…");

    const save = () => {
      window.clearTimeout(timer);
      if (!id) return;
      if (current === saved && !inFlight) {
        if (active) { setStatus("Saved"); setError(""); }
        return;
      }
      const key = id;
      const snapshot = current;
      if (active) { setStatus("Saving…"); setError(""); }
      const request = (pending.get(key) || Promise.resolve()).catch(() => {}).then(() => api.saveNotes(key, snapshot));
      pending.set(key, request);
      inFlight = request;
      request.then(() => {
        saved = snapshot;
        if (readDraft(key) === snapshot) {
          try { localStorage.removeItem(draftKey(key)); } catch { /* Optional local backup. */ }
        }
        if (active) setStatus(current === saved ? "Saved" : "Unsaved changes");
      }).catch(() => {
        if (active) { setStatus("Not saved"); setError("Could not save notes. Your edits are still here. Retry when connected."); }
      }).finally(() => {
        if (pending.get(key) === request) pending.delete(key);
        if (inFlight === request) inFlight = undefined;
      });
    };

    const load = async () => {
      if (notesId) await pending.get(notesId)?.catch(() => {});
      let notes = notesId ? await api.getNotes(notesId) : await api.getRecordingNotes(filename!);
      if (pending.has(notes.id)) {
        await pending.get(notes.id)?.catch(() => {});
        notes = await api.getNotes(notes.id);
      }
      if (!active) return;
      id = notes.id;
      saved = notes.text;
      current = readDraft(id) ?? saved;
      setText(current);
      setReady(true);
      setStatus(current === saved ? "Saved" : "Unsaved changes");
      setEdit({ change: value => {
        current = value;
        setText(value);
        storeDraft(id!, value);
        setStatus("Unsaved changes");
        window.clearTimeout(timer);
        timer = window.setTimeout(save, 600);
      }, save });
      if (current !== saved) save();
    };
    void load().catch(() => {
      if (active) { setStatus(""); setError("Could not load notes. Please retry."); }
    });
    const warn = (event: BeforeUnloadEvent) => {
      if (current !== saved || inFlight) { save(); event.preventDefault(); event.returnValue = ""; }
    };
    window.addEventListener("beforeunload", warn);
    return () => {
      active = false;
      window.clearTimeout(timer);
      window.removeEventListener("beforeunload", warn);
      save();
    };
  }, [notesId, filename, retry]);

  return <section class="notes-editor">
    <label for="custom-notes">Your notes</label>
    <p class="notes-hint">Add key points, questions, or action items. Notes save automatically with this recording.</p>
    <textarea id="custom-notes" value={text} disabled={!ready} rows={12}
      placeholder="Write your notes here…"
      onInput={event => edit?.change(event.currentTarget.value)} onBlur={() => edit?.save()} />
    <div class="notes-footer">
      <span role="status">{status}</span>
      <button class="btn btn-sm btn-save" disabled={!ready} onClick={() => edit?.save()}>Save notes</button>
    </div>
    {error && <p class="error" role="alert">{error}{" "}
      <button class="btn btn-sm btn-cancel" onClick={() => ready ? edit?.save() : setRetry(retry + 1)}>Retry</button>
    </p>}
  </section>;
}
