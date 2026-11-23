# 3. Transcription: batch, live captions and job lifecycle

Status: accepted

## Decisions

- **Batch** transcription mirrors what the official ElevenLabs SDK (2.68.0) sends: `model_id=scribe_v3`, `tag_audio_events=false`, `diarize=true`, a multipart POST to `/v1/speech-to-text` with the `xi-api-key` header, a 240 s timeout and two retries on 408/409/429/5xx.
- **Live captions** use the realtime WebSocket with `model_id=scribe_v3_live`, `audio_format=pcm_16000` and `commit_strategy=vad`, in chunks of at least 200 ms. Captions are ephemeral and source-labelled; they are discarded on stop and never stored as the transcript. The saved audio is transcribed separately.
- **Order of work.** The transcript is saved before any destructive audio cleanup. Transcript availability is independent of post-processing (title, cleanup), so a failed step never hides a saved transcript.
- **Failure reporting.** Job state is kept in memory so a job finishes even when the database cannot record the failure.
- **Startup.** If the database cannot open, the app stays available and retries every 5 s, closing the failed connection between attempts.
- **Listing.** A recording's file may disappear (automatic cleanup) while the list loads, so a missing file with a transcript or metadata is still listed.
