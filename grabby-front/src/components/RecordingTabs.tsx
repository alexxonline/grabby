import type { ComponentChildren } from "preact";
import { useState } from "preact/hooks";
import { NotesEditor } from "./NotesEditor";

export function RecordingTabs({ children, notesId, filename, initialNotes = false, live = false }: {
  children: ComponentChildren; notesId?: string; filename?: string; initialNotes?: boolean; live?: boolean;
}) {
  const [selected, setSelected] = useState(initialNotes ? 1 : 0);
  return <div class="recording-details">
    <div class="recording-tabs" role="tablist" aria-label="Recording details">
      {[live ? "Live transcript" : "Transcript", "Notes"].map((label, index) =>
        <button key={label} id={`detail-tab-${index}`} role="tab" aria-selected={selected === index}
          aria-controls={`detail-panel-${index}`} tabIndex={selected === index ? 0 : -1}
          onClick={() => setSelected(index)} onKeyDown={event => {
            if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
              event.preventDefault();
              const next = event.key === "Home" ? 0 : event.key === "End" ? 1 : 1 - selected;
              setSelected(next);
              document.getElementById(`detail-tab-${next}`)?.focus();
            }
          }}>{label}</button>)}
    </div>
    <div id="detail-panel-0" role="tabpanel" aria-labelledby="detail-tab-0" hidden={selected !== 0}>{children}</div>
    <div id="detail-panel-1" role="tabpanel" aria-labelledby="detail-tab-1" hidden={selected !== 1}>
      <NotesEditor key={notesId || filename} notesId={notesId} filename={filename} />
    </div>
  </div>;
}
