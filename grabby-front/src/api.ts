const API_BASE = "http://localhost:8000/api";

export interface RecordingStatus {
  recording: boolean;
  filename: string | null;
  started_at: string | null;
}

export interface RecordingInfo {
  filename: string;
  name: string;
  size_bytes: number;
  created_at: string;
}

export interface TranscriptionStatus {
  filename: string;
  status: "none" | "pending" | "completed" | "error";
  text?: string | null;
  language?: string | null;
  error?: string | null;
}

export interface SearchResult {
  filename: string;
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
};
