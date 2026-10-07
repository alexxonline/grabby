import { useEffect, useState } from "preact/hooks";
import { api } from "../api";
import type { SyncConflict, SyncSettings, SyncSnapshot } from "../api";

function Version({ label, value }: { label: string; value: SyncSnapshot }) {
  return <div><h3>{label}</h3><p>{value.deleted_at ? "Deleted recording" : value.title || value.filename}</p>
    <pre style={{ whiteSpace: "pre-wrap", maxHeight: "200px", overflow: "auto" }}>{value.transcription?.text || "No transcript"}</pre>
    <p>Notes: {value.notes?.text || "No notes"}</p></div>;
}

export function SyncSettingsPanel() {
  const [settings, setSettings] = useState<SyncSettings | null>(null);
  const [endpoint, setEndpoint] = useState("");
  const [key, setKey] = useState("");
  const [enabled, setEnabled] = useState(false);
  const [mock, setMock] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [conflicts, setConflicts] = useState<SyncConflict[]>([]);
  const refresh = async () => {
    const [status, versions] = await Promise.all([api.getSyncSettings(), api.getSyncConflicts()]);
    setSettings(status); setConflicts(versions);
    return status;
  };
  useEffect(() => {
    let active = true;
    api.getSyncSettings().then(s => {
      if (!active) return;
      setSettings(s); setEndpoint(s.endpoint); setEnabled(s.enabled); setMock(s.mock);
    }).catch(e => active && setError(e.message));
    api.getSyncConflicts().then(c => active && setConflicts(c)).catch(e => active && setError(e.message));
    const timer = window.setInterval(() => {
      Promise.all([api.getSyncSettings(), api.getSyncConflicts()]).then(([s, c]) => {
        if (active) { setSettings(s); setConflicts(c); }
      }).catch(() => {});
    }, 5000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  const act = async (operation: () => Promise<unknown>, success: string) => {
    setBusy(true); setError(""); setMessage("");
    try { await operation(); await refresh(); setMessage(success); }
    catch (e) { setError(e instanceof Error ? e.message : "Sync failed."); }
    finally { setBusy(false); }
  };
  const dirty = !!settings && (enabled !== settings.enabled || endpoint !== settings.endpoint || mock !== settings.mock || key !== "");
  return <section class="settings-card" style={{ marginTop: "24px" }}>
    <h2>Text sync</h2>
    <p>Sync titles, transcripts and notes across devices. Audio stays on this device. Enabled sync checks for changes every 30 seconds.</p>
    {error && <p class="error" role="alert">{error}</p>}
    {!settings ? <p>Loading sync settings…</p> : <>
      <fieldset disabled={busy} style={{ border: 0, padding: 0 }}>
        <p><label><input type="checkbox" checked={enabled} onChange={e => setEnabled(e.currentTarget.checked)} /> Enable sync</label></p>
        <p><label><input type="checkbox" checked={mock} onChange={e => setMock(e.currentTarget.checked)} /> Use local mock server for testing</label></p>
        <p>{mock ? "Test data stays in this app's isolated mock database tables." : "Enter your server's HTTPS receiving endpoint and API key."}</p>
        <p><label for="sync-endpoint">Receiving endpoint</label><br />
          <input id="sync-endpoint" type="url" placeholder="https://server.example/sync" value={endpoint} disabled={mock}
            style={{ width: "100%" }} onInput={e => setEndpoint(e.currentTarget.value)} /></p>
        <p><label for="sync-key">API key</label><br />
          <input id="sync-key" type="password" autoComplete="new-password" value={key}
            placeholder={settings.api_key_configured ? "Key saved; leave blank to keep it" : mock ? "Enter a test key" : "Required"}
            onInput={e => setKey(e.currentTarget.value)} /></p>
        <button onClick={() => act(async () => {
          await api.saveSyncSettings({ enabled, endpoint, mock, ...(key ? { api_key: key } : {}) }); setKey("");
        }, "Sync settings saved.")}>Save sync settings</button>{" "}
        <button disabled={!settings.enabled || dirty || settings.running} onClick={() => act(api.runSync, "Sync completed.")}>{mock ? "Test mock sync" : "Sync now"}</button>
      </fieldset>
      {settings.mock && settings.enabled && !dirty && <div>
        <p>Mock scenarios: prepare a test, then click Test mock sync. Failures happen once so the next attempt can recover.</p>
        {([['remote_edit', 'Simulate another device'], ['conflict', 'Simulate conflicting edits'], ['failure', 'Simulate server failure'], ['authentication', 'Simulate rejected key']] as const).map(([scenario, label]) =>
          <button style={{ marginRight: "8px", marginBottom: "8px" }} disabled={busy || settings.running} key={scenario}
            onClick={() => act(async () => { const result = await api.mockSyncScenario(scenario); setMessage(result.message); }, "Mock scenario prepared. Click Test mock sync.")}>{label}</button>)}
      </div>}
      {dirty && <p>Save settings before syncing.</p>}
      <p role="status">{busy || settings.running ? "Working…" : message || (settings.enabled ? "Sync enabled" : "Sync disabled")}</p>
      <p>Last successful sync: {settings.last_success ? new Date(settings.last_success).toLocaleString() : "Never"}</p>
      {settings.error && <p class="error">{settings.error}</p>}
      <small>Device: {settings.device_id}</small>
      {conflicts.map(c => <article key={c.recording_id}>
        <h3>Conflicting edits: {c.local.filename}</h3>
        <p>Choose the version to keep. Keeping this device's version uploads it on the next sync.</p>
        <Version label="This device" value={c.local} /><Version label="Server" value={c.server} />
        <button disabled={busy} onClick={() => act(() => api.resolveSyncConflict(c.recording_id, "local"), "Local version kept. Sync to upload it.")}>Keep this device</button>{" "}
        <button disabled={busy} onClick={() => act(() => api.resolveSyncConflict(c.recording_id, "server"), "Server version applied.")}>Keep server</button>
      </article>)}
    </>}
  </section>;
}
