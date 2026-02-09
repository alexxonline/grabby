import { useState, useEffect, useRef, useCallback } from "preact/hooks";

interface AudioPlayerProps {
  src: string;
  title: string;
  onClose: () => void;
}

export function AudioPlayer({ src, title, onClose }: AudioPlayerProps) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const progressRef = useRef<HTMLDivElement>(null);
  const [playing, setPlaying] = useState(false);
  const [currentTime, setCurrent] = useState(0);
  const [duration, setDuration] = useState(0);
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    const audio = audioRef.current!;
    const onLoaded = () => setDuration(audio.duration);
    const onTime = () => {
      if (!dragging) setCurrent(audio.currentTime);
    };
    const onEnded = () => setPlaying(false);

    audio.addEventListener("loadedmetadata", onLoaded);
    audio.addEventListener("timeupdate", onTime);
    audio.addEventListener("ended", onEnded);

    return () => {
      audio.removeEventListener("loadedmetadata", onLoaded);
      audio.removeEventListener("timeupdate", onTime);
      audio.removeEventListener("ended", onEnded);
      audio.pause();
    };
  }, [src, dragging]);

  const togglePlay = () => {
    const audio = audioRef.current!;
    if (playing) {
      audio.pause();
    } else {
      audio.play();
    }
    setPlaying(!playing);
  };

  const seekTo = useCallback(
    (clientX: number) => {
      const bar = progressRef.current!;
      const rect = bar.getBoundingClientRect();
      const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
      const time = ratio * duration;
      setCurrent(time);
      audioRef.current!.currentTime = time;
    },
    [duration]
  );

  const onPointerDown = (e: PointerEvent) => {
    setDragging(true);
    seekTo(e.clientX);
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent) => {
    if (!dragging) return;
    seekTo(e.clientX);
  };

  const onPointerUp = () => {
    setDragging(false);
  };

  const fmt = (s: number) => {
    if (!isFinite(s)) return "0:00";
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60);
    return `${m}:${sec.toString().padStart(2, "0")}`;
  };

  const pct = duration > 0 ? (currentTime / duration) * 100 : 0;

  return (
    <div class="audio-player">
      <audio ref={audioRef} src={src} preload="metadata" />

      <div class="ap-header">
        <span class="ap-title">{title}</span>
        <button class="ap-close" onClick={onClose} title="Close player">
          ✕
        </button>
      </div>

      <div class="ap-controls">
        <button class="ap-play" onClick={togglePlay} title={playing ? "Pause" : "Play"}>
          {playing ? (
            <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
              <rect x="6" y="4" width="4" height="16" rx="1" />
              <rect x="14" y="4" width="4" height="16" rx="1" />
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor">
              <path d="M8 5v14l11-7z" />
            </svg>
          )}
        </button>

        <span class="ap-time">{fmt(currentTime)}</span>

        <div
          class="ap-progress"
          ref={progressRef}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
        >
          <div class="ap-progress-track">
            <div class="ap-progress-fill" style={{ width: `${pct}%` }} />
            <div
              class="ap-progress-thumb"
              style={{ left: `${pct}%` }}
            />
          </div>
        </div>

        <span class="ap-time">{fmt(duration)}</span>
      </div>
    </div>
  );
}
