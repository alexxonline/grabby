# Text sync plan and API contract

## Implementation steps
1. Add stable recording UUIDs and a persistent device UUID; track UTC update times, local generations, server revisions and deletion tombstones. Backfill existing recordings. Group complete text snapshots by filename, with recording_id as the cross-device identity.
2. Add Settings controls: enable sync, HTTPS receiving endpoint, API key, explicit local mock mode, connection status, last success, Sync now, and conflict resolution. Store the API key only in backend .env as GRABBY_SYNC_API_KEY; never return it or sync it.
3. Exchange text snapshots in both directions. Poll every 30 seconds while enabled; retry safely, protect edits made during uploads, and preserve both conflicting versions until the user chooses one.
4. Provide a persistent local mock receiver using the same contract. Test multiple devices, retries, invalid keys, server failures, deletions, filename collisions and conflicts from the app.
5. Run backend regression tests and build the frontend.

## Synced fields
- recording_metadata: filename, title, created_at.
- transcriptions: text, language, created_at.
- recording_notes: id, text. Unattached notes wait for attachment.
- Additional fields: recording_id, device_id, updated_at, deleted_at, base_revision, mutation_id.

Exclude audio files/bytes/paths, local settings, API keys, processing jobs/errors and the FTS index. New local tables: sync_config, sync_control, sync_records, sync_conflicts, sync_mock_records and sync_mock_mutations. Triggers track edits; applying remote changes suppresses these triggers transactionally. Transcript search indexes rebuild via existing triggers. Synced metadata-only recordings appear in the library.

## Receiving endpoint
POST to the exact configured HTTPS URL. Headers: Content-Type: application/json and Authorization: Bearer <api-key>. Verify TLS certificates and reject redirects. The server scopes recordings, revisions and cursors to the authenticated account. The production receiver must implement this contract; this change provides a local reference receiver, not hosted infrastructure.

Request:
```json
{
  "schema_version": 1,
  "device_id": "44ea884b-b138-4bde-94f0-a60feefed617",
  "cursor": 0,
  "records": [{
    "recording_id": "930954bb-4292-4442-8c2b-b6b738b9dca9",
    "filename": "meeting.mp3",
    "created_at": "2026-10-05T14:00:00Z",
    "updated_at": "2026-10-05T14:35:00Z",
    "deleted_at": null,
    "base_revision": 0,
    "mutation_id": "930954bb-4292-4442-8c2b-b6b738b9dca9:1",
    "title": "Weekly planning",
    "transcription": {
      "text": "Alice: Let's review priorities.",
      "language": "en",
      "created_at": "2026-10-05T14:30:00Z"
    },
    "notes": {
      "id": "a7889f2e-7f27-41c9-9cf8-e97e923c9dc7",
      "text": "Follow up on the release date."
    }
  }]
}
```
Transcription and notes can be null. Title and language can be null. A deletion uses the same identity with a non-null deleted_at. UUIDs may be canonical or 32 hexadecimal digits. Legacy naive recording dates are interpreted in the device's local timezone; naive transcript dates as UTC. New transcript and metadata dates include timezone information.

Response:
```json
{
  "schema_version": 1,
  "cursor": 8,
  "acknowledgments": [{
    "recording_id": "930954bb-4292-4442-8c2b-b6b738b9dca9",
    "mutation_id": "930954bb-4292-4442-8c2b-b6b738b9dca9:1",
    "revision": 8
  }],
  "changes": [{
    "recording_id": "930954bb-4292-4442-8c2b-b6b738b9dca9",
    "filename": "meeting.mp3",
    "created_at": "2026-10-05T14:00:00Z",
    "updated_at": "2026-10-05T14:35:00Z",
    "deleted_at": null,
    "revision": 8,
    "title": "Weekly planning",
    "transcription": {
      "text": "Alice: Let's review priorities.",
      "language": "en",
      "created_at": "2026-10-05T14:30:00Z"
    },
    "notes": null
  }],
  "conflicts": []
}
```

## Server and client rules
- Each accepted mutation increments an account-wide revision; use it as the recording revision and response cursor.
- Deduplicate by device_id and mutation_id before checking base_revision. A replay returns the original acknowledgment without writing again.
- A new recording has base_revision=0. Reject stale revisions per record and include the current server snapshot in conflicts (same shape as changes). Unknown recordings with nonzero base_revision are invalid.
- Return every latest snapshot changed after the requested cursor, including tombstones and the sending device's accepted writes. Snapshot and cursor must be consistent; never advance beyond omitted changes. No pagination in v1. The client limits responses to 10 MB.
- HTTP 401: invalid key; 400: invalid payload/cursor; 5xx: transient failure. Keep unacknowledged local changes pending.
- If a locally edited record receives a newer remote revision, persist both versions. Keeping local rebases against the server revision and creates a fresh mutation; keeping server applies its snapshot.
- Filename collisions get a local UUID suffix. Identity always remains recording_id. Remote deletions hide synced text and never delete local audio.
- Changing endpoint or mock mode resets cursor/revisions and republishes local snapshots. API-key rotation keeps the namespace; use a distinct endpoint when switching accounts. Previously synchronized local snapshots remain local when changing servers.
- Sync is deferred while recording, saving or transcription is active. Local settings are never synchronized.

## Local API
- GET/PUT /api/sync/settings: enabled, endpoint, mock; PUT optionally accepts api_key. GET returns only api_key_configured plus device/status information.
- POST /api/sync/run: manual exchange.
- GET /api/sync/conflicts: local and server snapshots.
- POST /api/sync/conflicts/{recording_id}: {"choice":"local"} or {"choice":"server"}.
- POST /api/sync/mock/exchange: receiving contract above; loopback clients only, mock mode required, authenticates against the configured key.
- POST /api/sync/mock/scenario: {"scenario":"remote_edit"}, "conflict", "failure", or "authentication". Requires enabled mock sync.

The mock transport invokes the receiver directly without network access. It uses persistent isolated tables in the local database. The reference mock is a single test account; production hosting must keep it disabled or bind the app to loopback. HTTPS termination and production account authentication belong to the receiving server.

## Test from Settings
Open Settings > Text sync. Select local mock server, enter any test key, enable sync, and save. Click Test mock sync. Scenario buttons prepare another device's edit, conflicting edits, a one-time rejected key, or a one-time server failure. Click Test mock sync again to observe the result or recovery. These scenarios use local text snapshots; conflict choices apply to those recordings. Audio is never uploaded.

## Verification
Backend: `.venv\Scripts\python.exe -m unittest discover -s tests`.
Frontend: `npm run build`.
Tests cover text-only payloads, bidirectional exchange, conflict choices, deletions, retries after a lost response, edits during upload, filename collisions, endpoint/key validation, receiving authentication, mock scenarios, and migration stability. Use isolated recording/database directories when running tests so existing interrupted captures do not influence recording-state tests. Real HTTPS interoperability requires a deployed server and credentials; neither is configured by this implementation.
