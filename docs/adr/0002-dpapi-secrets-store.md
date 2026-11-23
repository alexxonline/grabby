# 2. API keys live in a DPAPI-encrypted store

Status: accepted

## Decision

API keys are encrypted with Windows DPAPI (`CryptProtectData`, current user) into `<data dir>\configs.dat`. The entropy binds each blob to its key name, so a blob copied under another name does not decrypt. The file holds only base64 blobs:

```json
{"version": 1, "secrets": {"ELEVENLABS_API_KEY": "<base64 DPAPI blob>"}}
```

- Plain-text values are never written to disk or shown in the UI. Keys typed in Settings are never displayed again, the input buffer is wiped, and decrypted copies live only as long as a request needs them.
- Keys come from the store or a real environment variable, never from a plain-text `.env`. At startup, keys found in the data folder's `.env` are moved into the store and those lines are removed.
- Reads and writes of the file are serialized by a lock.
- `.env` is read without mutating the process environment; real environment variables take precedence.
