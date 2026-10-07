import { useEffect, useState } from "preact/hooks";
import { api } from "../api";
import { SyncSettingsPanel } from "../components/SyncSettingsPanel";
import type { Settings } from "../api";

export function SettingsPage({ path: _path }: { path?: string }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(e => setError(e.message));
  }, []);

  const toggle = async (key: "auto_delete_audio" | "auto_title") => {
    if (!settings) return;
    setSaving(true);
    setSaved(false);
    setError("");
    try {
      setSettings(await api.updateSettings({
        auto_delete_audio: settings.auto_delete_audio,
        auto_title: settings.auto_title,
        [key]: !settings[key],
      }));
      setSaved(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not save settings.");
    } finally {
      setSaving(false);
    }
  };

  return <div class="page">
    <h1>Settings</h1>
    <p class="settings-intro">Choose what happens after your recordings are transcribed. Changes apply to future processing jobs.</p>
    {error && <p class="error" role="alert">{error}</p>}
    {!settings && !error && <p class="loading">Loading settings…</p>}
    {settings && <div class="settings-card">
      <div class="setting-row">
        <div>
          <label for="auto-delete">Auto-delete audio</label>
          <p id="auto-delete-help">Delete audio permanently after the transcript is safely saved. Keep the transcript in Recordings and Search. If transcription fails, audio is kept so you can retry.</p>
        </div>
        <input id="auto-delete" class="setting-toggle" type="checkbox" role="switch"
          aria-describedby="auto-delete-help" checked={settings.auto_delete_audio}
          disabled={saving} onChange={() => toggle("auto_delete_audio")} />
      </div>
      <div class="setting-row">
        <div>
          <label for="auto-title">Auto title</label>
          <p id="auto-title-help">Generate and apply a short title from the transcript using OpenAI GPT-5.6 Luna through OpenRouter. You can edit it with Rename. Enabling this sends transcript text to OpenRouter.</p>
          {!settings.openrouter_configured && <p class="settings-hint">Add OPENROUTER_API_KEY to grabby-backend/.env and restart the backend to generate titles.</p>}
        </div>
        <input id="auto-title" class="setting-toggle" type="checkbox" role="switch"
          aria-describedby="auto-title-help" checked={settings.auto_title}
          disabled={saving} onChange={() => toggle("auto_title")} />
      </div>
    </div>}
    <SyncSettingsPanel />
    <p role="status">{saving ? "Saving…" : saved ? "Settings saved." : ""}</p>
  </div>;
}
