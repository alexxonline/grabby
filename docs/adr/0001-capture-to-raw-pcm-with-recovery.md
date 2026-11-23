# 1. Capture to raw PCM files with a recovery manifest

Status: accepted

## Context

A meeting can run for hours, and the app or the machine can die mid-recording.

## Decisions

- **Raw PCM on disk.** Capture streams to raw PCM files in a hidden `.capture-<uuid>` folder next to a JSON `capture.json` manifest. This avoids unbounded RAM and WAV's 4 GiB limit. The streaming WAV writer patches its header sizes only when finished.
- **Recovery.** An interrupted capture is restored at startup, one at a time, from the manifest, so raw PCM formats are never guessed. A recovered capture has no known duration, so the UI only shows a length for a live recording.
- **System audio** is captured as an input stream on the default output (render) device (WASAPI loopback). WASAPI delivers no loopback packets while nothing plays, so the recorder writes silence to keep the system track aligned with the microphone.
- **Mixing.** Tracks are mixed over their overlap; past the end of one track the other plays alone. Streaming linear resampling keeps its position as a frame offset where `-1` is the previous call's last frame.
- **Stream errors.** Device switches, xruns and realtime-denied events are logged and do not end the meeting.
- **Saving.** Mixing and MP3 encoding happen in the capture folder, so intermediate or incomplete exports never appear in the recordings list. The finished path is written to the manifest before notes are attached, so a failed attach is retried without exporting a second file. Recovery data is deleted before the capture folder, so leftover PCM after a cleanup failure never makes a saved meeting look interrupted again. If `ffmpeg` is missing, WAV is kept.
- **File names.** Windows device names (`CON`, `COM1`, ...) stay reserved even with an extension, so they are rejected as recording names.
