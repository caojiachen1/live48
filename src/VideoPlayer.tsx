import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DismissRegular, PlayRegular, PauseRegular } from "@fluentui/react-icons";

interface VideoPlayerProps {
  liveId: string;
  title: string;
  nickname: string;
  streamUrl?: string;
  isLive?: boolean;
  onClose: () => void;
}

const isAndroid = /Android/i.test(navigator.userAgent);

function formatTime(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}

function VideoPlayer({ liveId, title, nickname, streamUrl: preUrl, isLive, onClose }: VideoPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const progressRef = useRef<HTMLDivElement>(null);
  const animFrameRef = useRef<number>(0);
  const [streamUrl, setStreamUrl] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>("");
  const [isPlaying, setIsPlaying] = useState(false);
  const [useCanvas, setUseCanvas] = useState(false);
  const hlsRef = useRef<any>(null);

  // Timeline state
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [buffered, setBuffered] = useState(0);
  const [isSeeking, setIsSeeking] = useState(false);
  const [showControls, setShowControls] = useState(true);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const flashControls = useCallback(() => {
    setShowControls(true);
    if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
    hideTimerRef.current = setTimeout(() => setShowControls(false), 3000);
  }, []);

  // Cleanup timer on unmount
  useEffect(() => {
    return () => {
      if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
    };
  }, []);

  // Canvas render loop
  const startCanvasLoop = useCallback(() => {
    const video = videoRef.current;
    const canvas = canvasRef.current;
    if (!video || !canvas) return;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const render = () => {
      if (!video || !canvas || !ctx) return;
      if (video.readyState >= 2 && !video.paused && !video.ended) {
        const vw = video.videoWidth;
        const vh = video.videoHeight;
        if (vw && vh) {
          if (canvas.width !== vw || canvas.height !== vh) {
            canvas.width = vw;
            canvas.height = vh;
          }
          ctx.drawImage(video, 0, 0, vw, vh);
        }
      }
      animFrameRef.current = requestAnimationFrame(render);
    };
    render();
  }, []);

  // Update timeline from video element
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    const onTimeUpdate = () => {
      if (!isSeeking) setCurrentTime(video.currentTime);
    };
    const onLoadedMetadata = () => setDuration(video.duration);
    const onProgress = () => {
      if (video.buffered.length > 0) {
        setBuffered(video.buffered.end(video.buffered.length - 1));
      }
    };
    const onPlay = () => setIsPlaying(true);
    const onPause = () => setIsPlaying(false);
    const onEnded = () => setIsPlaying(false);

    video.addEventListener("timeupdate", onTimeUpdate);
    video.addEventListener("loadedmetadata", onLoadedMetadata);
    video.addEventListener("progress", onProgress);
    video.addEventListener("play", onPlay);
    video.addEventListener("pause", onPause);
    video.addEventListener("ended", onEnded);

    return () => {
      video.removeEventListener("timeupdate", onTimeUpdate);
      video.removeEventListener("loadedmetadata", onLoadedMetadata);
      video.removeEventListener("progress", onProgress);
      video.removeEventListener("play", onPlay);
      video.removeEventListener("pause", onPause);
      video.removeEventListener("ended", onEnded);
    };
  }, [isSeeking]);

  // Load stream URL (skip fetch if already provided via props)
  useEffect(() => {
    const loadStream = async () => {
      try {
        setLoading(true);
        setError("");

        let url: string;
        if (preUrl) {
          url = preUrl;
        } else {
          url = await invoke("get_live_stream_url", { liveId });
        }
        setStreamUrl(url);

        if (url.startsWith("rtmp://") || url.startsWith("rtmps://")) {
          setError("RTMP 流无法在浏览器中直接播放，请使用外部播放器打开");
          return;
        }

        if (isAndroid && (url.includes(".m3u8") || isLive)) {
          setUseCanvas(true);
        }
      } catch (e) {
        console.error(e);
        setError("获取播放地址失败");
      } finally {
        setLoading(false);
      }
    };
    loadStream();
  }, [liveId, preUrl]);

  // Initialize player
  useEffect(() => {
    if (!streamUrl || !videoRef.current) return;

    const video = videoRef.current;
    let cancelled = false;

    const initPlayer = async () => {
      try {
        if (streamUrl.includes(".m3u8")) {
          const canPlayHls = video.canPlayType("application/vnd.apple.mpegurl") !== "";

          if (!canPlayHls) {
            const Hls = (await import("hls.js")).default;
            if (Hls.isSupported()) {
              const hls = new Hls({
                enableWorker: false,
                lowLatencyMode: false,
                maxBufferLength: 30,
                maxMaxBufferLength: 60,
                startLevel: -1,
                capLevelToPlayerSize: true,
              });
              hls.loadSource(streamUrl);
              hls.attachMedia(video);
              hls.on(Hls.Events.MANIFEST_PARSED, () => {
                if (!cancelled) {
                  video.play().then(() => {
                    if (useCanvas) startCanvasLoop();
                  }).catch((e) => {
                    console.error(e);
                    setError("播放失败，请尝试使用外部播放器");
                  });
                }
              });
              hls.on(Hls.Events.ERROR, (_, data) => {
                if (data.fatal) {
                  if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
                    hls.startLoad();
                  } else {
                    setError("播放失败：视频流错误");
                  }
                }
              });
              hlsRef.current = hls;
            } else {
              setError("当前浏览器不支持 HLS 播放");
            }
          } else {
            video.src = streamUrl;
            video.play().then(() => {
              if (useCanvas) startCanvasLoop();
            }).catch((e) => {
              console.error(e);
              setError("播放失败，请尝试使用外部播放器");
            });
          }
        } else {
          video.src = streamUrl;
          video.play().then(() => {
            if (useCanvas) startCanvasLoop();
          }).catch((e) => {
            console.error(e);
            setError("播放失败，请尝试使用外部播放器");
          });
        }
      } catch (e) {
        console.error(e);
        setError("播放初始化失败");
      }
    };

    initPlayer();

    return () => {
      cancelled = true;
      if (animFrameRef.current) {
        cancelAnimationFrame(animFrameRef.current);
        animFrameRef.current = 0;
      }
      if (hlsRef.current) {
        hlsRef.current.destroy();
        hlsRef.current = null;
      }
      video.pause();
      video.removeAttribute("src");
      video.load();
    };
  }, [streamUrl, useCanvas, startCanvasLoop]);

  // Play/Pause toggle
  const togglePlay = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) {
      video.play().catch(console.error);
    } else {
      video.pause();
    }
  }, []);

  // Seek handling
  const handleSeekStart = useCallback((e: React.MouseEvent | React.TouchEvent) => {
    e.preventDefault();
    setIsSeeking(true);
  }, []);

  const handleSeekMove = useCallback((clientX: number) => {
    const bar = progressRef.current;
    const video = videoRef.current;
    if (!bar || !video || !duration) return;

    const rect = bar.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    const seekTime = ratio * duration;
    setCurrentTime(seekTime);
  }, [duration]);

  const handleSeekEnd = useCallback((clientX: number) => {
    const bar = progressRef.current;
    const video = videoRef.current;
    if (!bar || !video || !duration) {
      setIsSeeking(false);
      return;
    }

    const rect = bar.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    video.currentTime = ratio * duration;
    setIsSeeking(false);
  }, [duration]);

  // Touch events for seek bar
  const handleTouchMove = useCallback((e: React.TouchEvent) => {
    if (isSeeking && e.touches[0]) {
      handleSeekMove(e.touches[0].clientX);
    }
  }, [isSeeking, handleSeekMove]);

  const handleTouchEnd = useCallback((e: React.TouchEvent) => {
    if (isSeeking && e.changedTouches[0]) {
      handleSeekEnd(e.changedTouches[0].clientX);
    }
  }, [isSeeking, handleSeekEnd]);

  // Mouse move on progress bar
  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (isSeeking) handleSeekMove(e.clientX);
  }, [isSeeking, handleSeekMove]);

  const handleMouseUp = useCallback((e: React.MouseEvent) => {
    if (isSeeking) handleSeekEnd(e.clientX);
  }, [isSeeking, handleSeekEnd]);

  // Toggle controls on video area click, auto-hide after 3s when shown
  const handleVideoClick = useCallback((e: React.MouseEvent) => {
    e.stopPropagation();
    if (showControls) {
      setShowControls(false);
      if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
    } else {
      flashControls();
    }
  }, [showControls, flashControls]);

  // Open in external player
  const openExternally = useCallback(async () => {
    if (streamUrl) {
      try {
        await invoke("open_in_player", { url: streamUrl });
      } catch (e) {
        console.error(e);
      }
    }
  }, [streamUrl]);

  // Close overlay when clicking background (not video area)
  const handleOverlayClick = (e: React.MouseEvent) => {
    if (e.target === containerRef.current) onClose();
  };

  // Escape key
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const isSeekable = isFinite(duration) && duration > 0;
  const progressRatio = isSeekable ? (currentTime / duration) * 100 : 0;
  const bufferedRatio = isSeekable ? (buffered / duration) * 100 : 0;

  return (
    <div
      ref={containerRef}
      onClick={handleOverlayClick}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        backgroundColor: "#000",
        zIndex: 9999,
        display: "flex",
        flexDirection: "column",
        userSelect: "none",
        WebkitUserSelect: "none",
      }}
    >
      {/* Top bar — always visible, above video */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          padding: "24px 16px 10px 16px",
          flexShrink: 0,
        }}
      >
        <button
          onClick={onClose}
          style={{
            background: "none",
            border: "none",
            color: "white",
            cursor: "pointer",
            padding: "4px",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
          }}
          aria-label="关闭播放"
        >
          <DismissRegular style={{ fontSize: "24px" }} />
        </button>
        <div style={{ marginLeft: "8px", flex: 1, overflow: "hidden" }}>
          <div style={{
            color: "white", fontSize: "15px", fontWeight: 600,
            whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis"
          }}>
            {title || "无标题"}
          </div>
          <div style={{ color: "rgba(255,255,255,0.6)", fontSize: "12px", marginTop: "1px" }}>
            {nickname}
          </div>
        </div>
      </div>

      {/* Video area — fills remaining space */}
      <div
        onClick={handleVideoClick}
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          overflow: "hidden",
          position: "relative",
          minHeight: 0,
        }}
      >
        {/* Loading */}
        {loading && (
          <div style={{ textAlign: "center", color: "rgba(255,255,255,0.7)", zIndex: 5 }}>
            <div
              className="player-spinner"
              style={{
                width: "48px", height: "48px",
                border: "3px solid rgba(255,255,255,0.2)",
                borderTop: "3px solid #bb86fc",
                borderRadius: "50%",
                animation: "player-spin 0.8s linear infinite",
                margin: "0 auto 16px auto",
              }}
            />
            <div>正在加载视频流...</div>
          </div>
        )}

        {/* Error */}
        {error && (
          <div style={{ textAlign: "center", color: "#ff6b6b", padding: "20px", zIndex: 5 }}>
            <div style={{ fontSize: "48px", marginBottom: "12px" }}>!</div>
            <div style={{ marginBottom: "16px" }}>{error}</div>
            <div style={{ display: "flex", gap: "12px", justifyContent: "center" }}>
              {(streamUrl.startsWith("rtmp://") || streamUrl.startsWith("rtmps://")) && (
                <button
                  onClick={openExternally}
                  style={{
                    padding: "10px 24px",
                    backgroundColor: "#bb86fc",
                    color: "white",
                    border: "none",
                    borderRadius: "8px",
                    fontSize: "16px",
                    cursor: "pointer",
                  }}
                >
                  用外部播放器打开
                </button>
              )}
              <button
                onClick={onClose}
                style={{
                  padding: "10px 24px",
                  backgroundColor: "rgba(255,255,255,0.15)",
                  color: "white",
                  border: "none",
                  borderRadius: "8px",
                  fontSize: "16px",
                  cursor: "pointer",
                }}
              >
                关闭
              </button>
            </div>
          </div>
        )}

        {/* Canvas (Android distortion workaround) */}
        <canvas
          ref={canvasRef}
          style={{
            display: useCanvas && !loading && !error ? "block" : "none",
            maxWidth: "100%",
            maxHeight: "100%",
            objectFit: "contain",
            backgroundColor: "#000",
          }}
        />

        {/* Video element */}
        <video
          ref={videoRef}
          playsInline
          crossOrigin="anonymous"
          style={{
            display: loading || error ? "none" : useCanvas ? "none" : "block",
            maxWidth: "100%",
            maxHeight: "100%",
            width: "auto",
            height: "auto",
            backgroundColor: "#000",
            objectFit: "contain",
          }}
        />

        {/* Center play/pause icon — no background box, shows when controls visible or paused */}
        {!loading && !error && (
          <div
            onClick={(e) => { e.stopPropagation(); togglePlay(); }}
            style={{
              position: "absolute",
              top: "50%",
              left: "50%",
              transform: "translate(-50%, -50%)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              opacity: showControls || !isPlaying ? 0.9 : 0,
              transition: "opacity 0.25s ease",
              cursor: "pointer",
              zIndex: 10,
              pointerEvents: showControls || !isPlaying ? "auto" : "none",
            }}
          >
            {isPlaying ? (
              <PauseRegular style={{ fontSize: "40px", color: "white", filter: "drop-shadow(0 1px 3px rgba(0,0,0,0.6))" }} />
            ) : (
              <PlayRegular style={{ fontSize: "40px", color: "white", filter: "drop-shadow(0 1px 3px rgba(0,0,0,0.6))" }} />
            )}
          </div>
        )}
      </div>

      {/* Bottom controls — below video, auto-hides after 3s */}
      {!loading && !error && (
        <div
          style={{
            padding: "0 16px 20px 16px",
            flexShrink: 0,
            opacity: showControls ? 1 : 0,
            transition: "opacity 0.25s ease",
            pointerEvents: showControls ? "auto" : "none",
          }}
        >
          {/* Progress bar — only for seekable (VOD) content */}
          {isSeekable ? (
            <div
              ref={progressRef}
              onMouseDown={handleSeekStart}
              onTouchStart={handleSeekStart}
              onTouchMove={handleTouchMove}
              onTouchEnd={handleTouchEnd}
              style={{
                width: "100%",
                height: "20px",
                padding: "8px 0",
                cursor: "pointer",
                position: "relative",
                touchAction: "none",
              }}
            >
              <div style={{
                position: "relative",
                width: "100%",
                height: "4px",
                backgroundColor: "rgba(255,255,255,0.15)",
                borderRadius: "2px",
              }}>
                <div style={{
                  position: "absolute", left: 0, top: 0, height: "100%",
                  width: `${bufferedRatio}%`,
                  backgroundColor: "rgba(255,255,255,0.25)", borderRadius: "2px",
                }} />
                <div style={{
                  position: "absolute", left: 0, top: 0, height: "100%",
                  width: `${progressRatio}%`,
                  backgroundColor: "#bb86fc", borderRadius: "2px",
                  transition: isSeeking ? "none" : "width 0.15s linear",
                }} />
                <div style={{
                  position: "absolute", left: `${progressRatio}%`, top: "50%",
                  transform: "translate(-50%, -50%)",
                  width: isSeeking ? "14px" : "10px",
                  height: isSeeking ? "14px" : "10px",
                  borderRadius: "50%", backgroundColor: "#bb86fc",
                  transition: isSeeking ? "none" : "left 0.15s linear, width 0.15s, height 0.15s",
                }} />
              </div>
            </div>
          ) : (
            <div style={{ height: "20px", display: "flex", alignItems: "center", justifyContent: "center" }}>
              <div style={{
                display: "flex", alignItems: "center", gap: "6px",
                color: "#ff4444", fontSize: "12px", fontWeight: 700,
                letterSpacing: "1px",
              }}>
                <span style={{
                  width: "8px", height: "8px", borderRadius: "50%",
                  backgroundColor: "#ff4444",
                  animation: "player-live-pulse 1.5s ease-in-out infinite",
                }} />
                直播
              </div>
            </div>
          )}

          {/* Time and play/pause row */}
          <div style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            marginTop: "2px",
          }}>
            <button
              onClick={togglePlay}
              style={{
                background: "none", border: "none", color: "white",
                cursor: "pointer", padding: "4px",
                display: "flex", alignItems: "center", justifyContent: "center",
              }}
              aria-label={isPlaying ? "暂停" : "播放"}
            >
              {isPlaying ? (
                <PauseRegular style={{ fontSize: "22px" }} />
              ) : (
                <PlayRegular style={{ fontSize: "22px" }} />
              )}
            </button>

            <div style={{
              color: "rgba(255,255,255,0.75)", fontSize: "12px",
              fontVariantNumeric: "tabular-nums",
              display: "flex", alignItems: "center", gap: "4px",
            }}>
              {isSeekable ? (
                <>
                  <span>{formatTime(currentTime)}</span>
                  <span style={{ color: "rgba(255,255,255,0.35)" }}>/</span>
                  <span>{formatTime(duration)}</span>
                </>
              ) : (
                <span>{formatTime(currentTime)}</span>
              )}
            </div>
          </div>
        </div>
      )}

      <style>{`
        @keyframes player-spin {
          0% { transform: rotate(0deg); }
          100% { transform: rotate(360deg); }
        }
        @keyframes player-live-pulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.3; }
        }
      `}</style>
    </div>
  );
}

export default VideoPlayer;
