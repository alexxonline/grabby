import type { Page, Route } from "@playwright/test";

export interface MockOptions {
  library?: "seeded" | "empty";
  statusFails?: boolean;
}

interface Recording {
  filename: string;
  name: string;
  size_bytes: number;
  created_at: string;
  audio_available: boolean;
  processing: boolean;
  processing_error: string | null;
}

interface Transcript {
  filename: string;
  title: string | null;
  status: "none" | "pending" | "completed" | "error";
  text: string | null;
  language: string | null;
  error: string | null;
  processing: boolean;
}

interface Notes {
  id: string;
  text: string;
}

interface Settings {
  auto_delete_audio: boolean;
  auto_title: boolean;
  openrouter_configured: boolean;
  title_model: string;
}

interface RecorderStatus {
  recording: boolean;
  save_pending: boolean;
  saving: boolean;
  error: string | null;
  filename: string | null;
  started_at: string | null;
  notes_id: string | null;
  live: {
    segments: { source: "you" | "others"; text: string }[];
    sources: Record<"you" | "others", { status: string; partial: string; error: string | null }>;
  } | null;
}

const JSON_HEADERS = {
  "access-control-allow-origin": "*",
  "access-control-allow-headers": "content-type",
  "access-control-allow-methods": "GET, POST, PUT, PATCH, DELETE, OPTIONS",
  "content-type": "application/json",
};

const PLANNING_TEXT = "Alice: We should ship the notes editor.\nBob: Sounds good.";
const STANDUP_TEXT = "Chris: Standup is done.";

function silentWav(): Buffer {
  const numSamples = 800;
  const dataSize = numSamples * 2;
  const wav = Buffer.alloc(44 + dataSize);
  wav.write("RIFF", 0);
  wav.writeUInt32LE(36 + dataSize, 4);
  wav.write("WAVE", 8);
  wav.write("fmt ", 12);
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(1, 22);
  wav.writeUInt32LE(8000, 24);
  wav.writeUInt32LE(16000, 28);
  wav.writeUInt16LE(2, 32);
  wav.writeUInt16LE(16, 34);
  wav.write("data", 36);
  wav.writeUInt32LE(dataSize, 40);
  return wav;
}

function slug(name: string): string {
  const stem = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  return stem || "meeting";
}

function snippet(text: string, terms: string[]): string {
  const flat = text.replace(/\s+/g, " ");
  const lower = flat.toLowerCase();
  const first = terms[0] ?? "";
  const at = lower.indexOf(first);
  const start = Math.max(0, at - 24);
  const end = Math.min(flat.length, (at < 0 ? 0 : at) + first.length + 24);
  let slice = `${start > 0 ? "…" : ""}${flat.slice(start, end)}${end < flat.length ? "…" : ""}`;
  for (const term of terms) {
    const pattern = new RegExp(term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "ig");
    slice = slice.replace(pattern, (match) => `**${match}**`);
  }
  return slice;
}

function createLibrary(): { recordings: Recording[]; transcripts: Record<string, Transcript>; notes: Record<string, Notes>; notesByFilename: Record<string, string> } {
  const recordings: Recording[] = [
    {
      filename: "planning.mp3",
      name: "Sprint planning",
      size_bytes: 1_572_864,
      created_at: "2026-09-20T14:30:00.000Z",
      audio_available: true,
      processing: false,
      processing_error: null,
    },
    {
      filename: "interview.wav",
      name: "Customer interview",
      size_bytes: 524_288,
      created_at: "2026-09-18T11:00:00.000Z",
      audio_available: true,
      processing: false,
      processing_error: null,
    },
    {
      filename: "standup.mp3",
      name: "Standup",
      size_bytes: 0,
      created_at: "2026-09-01T09:00:00.000Z",
      audio_available: false,
      processing: false,
      processing_error: null,
    },
  ];
  const transcripts: Record<string, Transcript> = {
    "planning.mp3": {
      filename: "planning.mp3",
      title: "Sprint planning",
      status: "completed",
      text: PLANNING_TEXT,
      language: "en",
      error: null,
      processing: false,
    },
    "standup.mp3": {
      filename: "standup.mp3",
      title: "Standup",
      status: "completed",
      text: STANDUP_TEXT,
      language: "en",
      error: null,
      processing: false,
    },
  };
  const notes: Record<string, Notes> = {
    "notes-planning": { id: "notes-planning", text: "Bring the roadmap." },
  };
  return {
    recordings,
    transcripts,
    notes,
    notesByFilename: { "planning.mp3": "notes-planning" },
  };
}

export async function installGrabbyApi(page: Page, options: MockOptions = {}): Promise<void> {
  const library = options.library === "empty" ? {
    recordings: [] as Recording[],
    transcripts: {} as Record<string, Transcript>,
    notes: {} as Record<string, Notes>,
    notesByFilename: {} as Record<string, string>,
  } : createLibrary();
  const recordings = library.recordings;
  const transcripts = library.transcripts;
  const notes = library.notes;
  const notesByFilename = library.notesByFilename;
  const settings: Settings = {
    auto_delete_audio: false,
    auto_title: false,
    openrouter_configured: false,
    title_model: "openai/gpt-5.6-luna",
  };
  let pendingName = "meeting";
  let status: RecorderStatus = {
    recording: false,
    save_pending: false,
    saving: false,
    error: null,
    filename: null,
    started_at: null,
    notes_id: null,
    live: null,
  };

  const recordingByName = (filename: string) => recordings.find((item) => item.filename === filename);

  const sortedRecordings = () => [...recordings].sort((a, b) => b.created_at.localeCompare(a.created_at));

  const notesFor = (filename: string): Notes => {
    const existing = notesByFilename[filename];
    if (existing && notes[existing]) return notes[existing];
    const id = `notes-${filename}`;
    notesByFilename[filename] = id;
    notes[id] = { id, text: "" };
    return notes[id];
  };

  const transcriptFor = (filename: string): Transcript => transcripts[filename] ?? {
    filename,
    title: recordingByName(filename)?.name ?? null,
    status: "none",
    text: null,
    language: null,
    error: null,
    processing: false,
  };

  const readJson = (route: Route): Record<string, unknown> => {
    const raw = route.request().postData();
    if (!raw) return {};
    return JSON.parse(raw) as Record<string, unknown>;
  };

  const respond = (route: Route): { status: number; body: string | Buffer; contentType: string } => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();

    if (options.statusFails && path === "/api/status") {
      return { status: 503, contentType: "application/json", body: JSON.stringify({ detail: "Recorder unavailable" }) };
    }

    if (method === "GET" && path === "/api/status") {
      return { status: 200, contentType: "application/json", body: JSON.stringify(status) };
    }

    if (method === "POST" && path === "/api/start") {
      if (status.recording) {
        return { status: 409, contentType: "application/json", body: JSON.stringify({ detail: "Already recording" }) };
      }
      const body = readJson(route);
      const name = typeof body.name === "string" ? body.name.trim() : "";
      pendingName = name || "meeting";
      const id = "notes-live";
      notes[id] = { id, text: "" };
      status = {
        recording: true,
        save_pending: false,
        saving: false,
        error: null,
        filename: slug(pendingName),
        started_at: new Date().toISOString(),
        notes_id: id,
        live: {
          segments: [{ source: "you", text: "Hello from the test." }],
          sources: {
            you: { status: "listening", partial: "", error: null },
            others: { status: "listening", partial: "joining now", error: null },
          },
        },
      };
      return { status: 200, contentType: "application/json", body: JSON.stringify(status) };
    }

    if (method === "POST" && path === "/api/stop") {
      if (!status.recording) {
        return { status: 409, contentType: "application/json", body: JSON.stringify({ detail: "Not recording" }) };
      }
      const filename = `${status.filename}.mp3`;
      recordings.unshift({
        filename,
        name: pendingName,
        size_bytes: 32_000,
        created_at: new Date().toISOString(),
        audio_available: true,
        processing: false,
        processing_error: null,
      });
      status = {
        recording: false,
        save_pending: false,
        saving: false,
        error: null,
        filename,
        started_at: null,
        notes_id: status.notes_id,
        live: null,
      };
      return { status: 200, contentType: "application/json", body: JSON.stringify(status) };
    }

    if (method === "GET" && path === "/api/settings") {
      return { status: 200, contentType: "application/json", body: JSON.stringify(settings) };
    }

    if (method === "PUT" && path === "/api/settings") {
      const body = readJson(route);
      settings.auto_delete_audio = Boolean(body.auto_delete_audio);
      settings.auto_title = Boolean(body.auto_title);
      return { status: 200, contentType: "application/json", body: JSON.stringify(settings) };
    }

    if (method === "GET" && path === "/api/recordings") {
      return { status: 200, contentType: "application/json", body: JSON.stringify(sortedRecordings()) };
    }

    if (method === "GET" && path === "/api/transcriptions/search") {
      const terms = (url.searchParams.get("q") ?? "").trim().toLowerCase().split(/\s+/).filter(Boolean);
      const results = terms.length === 0 ? [] : Object.values(transcripts).flatMap((item) => {
        if (item.status !== "completed" || !item.text) return [];
        const haystack = item.text.toLowerCase();
        if (!terms.every((term) => haystack.includes(term))) return [];
        return [{
          filename: item.filename,
          title: recordingByName(item.filename)?.name ?? item.title,
          snippet: snippet(item.text, terms),
          language: item.language,
        }];
      });
      return { status: 200, contentType: "application/json", body: JSON.stringify(results) };
    }

    const notesMatch = path.match(/^\/api\/notes\/([^/]+)$/);
    if (notesMatch) {
      const id = decodeURIComponent(notesMatch[1]);
      if (method === "GET") {
        const found = notes[id];
        if (!found) return { status: 404, contentType: "application/json", body: JSON.stringify({ detail: "Notes not found" }) };
        return { status: 200, contentType: "application/json", body: JSON.stringify(found) };
      }
      if (method === "PUT") {
        const found = notes[id];
        if (!found) return { status: 404, contentType: "application/json", body: JSON.stringify({ detail: "Notes not found" }) };
        const body = readJson(route);
        found.text = typeof body.text === "string" ? body.text : "";
        return { status: 200, contentType: "application/json", body: JSON.stringify(found) };
      }
    }

    const recordingMatch = path.match(/^\/api\/recordings\/([^/]+?)(?:\/(notes|transcription|transcribe|download))?$/);
    if (recordingMatch) {
      const filename = decodeURIComponent(recordingMatch[1]);
      const action = recordingMatch[2] ?? "";
      const recording = recordingByName(filename);
      if (!recording && action !== "download") {
        return { status: 404, contentType: "application/json", body: JSON.stringify({ detail: "Recording not found" }) };
      }

      if (method === "GET" && action === "") {
        return { status: 200, contentType: "application/json", body: JSON.stringify(recording) };
      }
      if (method === "DELETE" && action === "") {
        const index = recordings.findIndex((item) => item.filename === filename);
        if (index >= 0) recordings.splice(index, 1);
        delete transcripts[filename];
        const notesId = notesByFilename[filename];
        if (notesId) delete notes[notesId];
        delete notesByFilename[filename];
        return { status: 200, contentType: "application/json", body: JSON.stringify({ detail: "deleted" }) };
      }
      if (method === "PATCH" && action === "") {
        const body = readJson(route);
        const title = typeof body.new_name === "string" ? body.new_name.trim() : "";
        if (!title || title.length > 160 || !recording) {
          return { status: 400, contentType: "application/json", body: JSON.stringify({ detail: "Title must contain 1 to 160 characters" }) };
        }
        recording.name = title;
        if (transcripts[filename]) transcripts[filename].title = title;
        return { status: 200, contentType: "application/json", body: JSON.stringify(recording) };
      }
      if (method === "GET" && action === "notes") {
        return { status: 200, contentType: "application/json", body: JSON.stringify(notesFor(filename)) };
      }
      if (method === "GET" && action === "transcription") {
        return { status: 200, contentType: "application/json", body: JSON.stringify(transcriptFor(filename)) };
      }
      if (method === "PUT" && action === "transcription") {
        const existing = transcripts[filename];
        if (!existing || existing.status !== "completed") {
          return { status: 404, contentType: "application/json", body: JSON.stringify({ detail: "Transcription not found" }) };
        }
        const body = readJson(route);
        existing.text = typeof body.text === "string" ? body.text : "";
        return { status: 200, contentType: "application/json", body: JSON.stringify(existing) };
      }
      if (method === "POST" && action === "transcribe") {
        if (transcripts[filename]?.status === "completed") {
          return { status: 200, contentType: "application/json", body: JSON.stringify(transcripts[filename]) };
        }
        if (!recording?.audio_available) {
          return { status: 404, contentType: "application/json", body: JSON.stringify({ detail: "Audio not found" }) };
        }
        transcripts[filename] = {
          filename,
          title: recording.name,
          status: "completed",
          text: "Alex: Thanks for joining.\nSam: Happy to be here.",
          language: "en",
          error: null,
          processing: false,
        };
        return { status: 200, contentType: "application/json", body: JSON.stringify(transcripts[filename]) };
      }
      if (method === "GET" && action === "download") {
        if (!recording?.audio_available) {
          return { status: 404, contentType: "application/json", body: JSON.stringify({ detail: "Recording not found" }) };
        }
        return { status: 200, contentType: "audio/wav", body: silentWav() };
      }
    }

    return { status: 404, contentType: "application/json", body: JSON.stringify({ detail: "Not found" }) };
  };

  const handle = async (route: Route) => {
    if (route.request().method() === "OPTIONS") {
      await route.fulfill({ status: 204, headers: JSON_HEADERS });
      return;
    }
    let result: { status: number; body: string | Buffer; contentType: string };
    try {
      result = respond(route);
    } catch (error) {
      result = {
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ detail: error instanceof Error ? error.message : "Mock failed" }),
      };
    }
    await route.fulfill({
      status: result.status,
      headers: {
        ...JSON_HEADERS,
        "content-type": result.contentType,
      },
      body: result.body,
    });
  };

  let tail = Promise.resolve();
  const queued = (route: Route) => {
    const task = tail.then(() => handle(route));
    tail = task.then(() => undefined, () => undefined);
    return task;
  };

  await page.route("http://127.0.0.1:8000/**", queued);
  await page.route("http://localhost:8000/**", queued);
}
