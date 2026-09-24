const API_BASE = "http://localhost:8000/api";

export interface RecordingStatus {
  recording: boolean;
  save_pending?: boolean;
  saving?: boolean;
  error?: string | null;
  filename: string | null;
  started_at: string | null;
  notes_id?: string | null;
  live?: {
    segments: { source: "you" | "others"; text: string }[];
    sources: Record<"you" | "others", {
      status: string; partial: string; error: string | null;
    }>;
  } | null;
}

export interface RecordingInfo {
  filename: string;
  name: string;
  size_bytes: number;
  created_at: string;
  audio_available: boolean;
  processing: boolean;
  processing_error: string | null;
}

export interface Settings {
  auto_delete_audio: boolean;
  auto_title: boolean;
  openrouter_configured: boolean;
  title_model: string;
}

export interface TranscriptionStatus {
  filename: string;
  title?: string | null;
  processing?: boolean;
  status: "none" | "pending" | "completed" | "error";
  text?: string | null;
  language?: string | null;
  error?: string | null;
}

export interface SearchResult {
  filename: string;
  title?: string | null;
  snippet: string;
  language?: string | null;
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ detail: res.statusText }));
    throw new Error(err.detail || res.statusText);
  }
  return res.json();
}

export const api = {
  getNotes: (id: string) => request<{ id: string; text: string }>(`/notes/${encodeURIComponent(id)}`),
  getRecordingNotes: (filename: string) =>
    request<{ id: string; text: string }>(`/recordings/${encodeURIComponent(filename)}/notes`),
  saveNotes: (id: string, text: string) =>
    request<{ id: string; text: string }>(`/notes/${encodeURIComponent(id)}`, {
      method: "PUT", body: JSON.stringify({ text }),
    }),
  getSettings: () => request<Settings>("/settings"),
  updateSettings: (settings: Pick<Settings, "auto_delete_audio" | "auto_title">) =>
    request<Settings>("/settings", { method: "PUT", body: JSON.stringify(settings) }),
  getStatus: () => request<RecordingStatus>("/status"),

  startRecording: (name?: string) =>
    request<RecordingStatus>("/start", {
      method: "POST",
      body: JSON.stringify({ name: name || null }),
    }),

  stopRecording: () =>
    request<RecordingStatus>("/stop", { method: "POST" }),

  listRecordings: () => request<RecordingInfo[]>("/recordings"),

  deleteRecording: (filename: string) =>
    request<{ detail: string }>(`/recordings/${encodeURIComponent(filename)}`, {
      method: "DELETE",
    }),

  renameRecording: (filename: string, newName: string) =>
    request<RecordingInfo>(`/recordings/${encodeURIComponent(filename)}`, {
      method: "PATCH",
      body: JSON.stringify({ new_name: newName }),
    }),

  downloadUrl: (filename: string) =>
    `${API_BASE}/recordings/${encodeURIComponent(filename)}/download`,

  transcribeRecording: (filename: string) =>
    request<TranscriptionStatus>(
      `/recordings/${encodeURIComponent(filename)}/transcribe`,
      { method: "POST" }
    ),

  getTranscription: (filename: string) =>
    request<TranscriptionStatus>(
      `/recordings/${encodeURIComponent(filename)}/transcription`
    ),

  searchTranscriptions: (query: string) =>
    request<SearchResult[]>(
      `/transcriptions/search?q=${encodeURIComponent(query)}`
    ),

  updateTranscription: (filename: string, text: string) =>
    request<TranscriptionStatus>(
      `/recordings/${encodeURIComponent(filename)}/transcription`,
      { method: "PUT", body: JSON.stringify({ text }) }
    ),
};
