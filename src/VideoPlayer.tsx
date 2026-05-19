import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DismissRegular, PlayRegular, PauseRegular } from "@fluentui/react-icons";

interface VideoPlayerProps {
  liveId: string;
  title: string;
  nickname: string;
  streamUrl?: string;
  isLive?: boolean;
  onSwitch?: (direction: "prev" | "next") => void | Promise<void>;
  onClose: () => void;
}

const isAndroid = /Android/i.test(navigator.userAgent);

interface NativeAndroidPlayerBridge {
  isAvailable?: () => boolean;
  playStream: (url: string, title: string, nickname: string, isLive: boolean) => void;
}

declare global {
  interface Window {
    Live48NativePlayer?: NativeAndroidPlayerBridge;
  }
}

function formatTime(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}

function VideoPlayer({ liveId, title, nickname, streamUrl: preUrl, isLive, onSwitch, onClose }: VideoPlayerProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const progressRef = useRef<HTMLDivElement>(null);
  const animFrameRef = useRef<number>(0);
  const retryCountRef = useRef<number>(0);
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
  const [showControls, setShowControls] = useState(false);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const swipeStartRef = useRef<{ x: number; y: number } | null>(null);
  const switchingRef = useRef(false);
  const [switchStyle, setSwitchStyle] = useState<{
    transform: string;
    opacity: number;
    transition: string;
  }>({
    transform: "translateY(0)",
    opacity: 1,
    transition: "transform 0ms linear, opacity 0ms linear",
  });

  const tryOpenNativeAndroidPlayer = useCallback((url: string) => {
    if (!isAndroid) return false;
    const bridge = window.Live48NativePlayer;
    if (!bridge || typeof bridge.playStream !== "function") return false;
    if (typeof bridge.isAvailable === "function" && !bridge.isAvailable()) return false;
    bridge.playStream(url, title || "", nickname || "", Boolean(isLive));
    return true;
  }, [title, nickname, isLive]);

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

  // Load stream URL
  useEffect(() => {
    let cancelled = false;

    const loadStream = async () => {
      try {
        if (!cancelled) {
          setLoading(true);
          setError("");
        }

        let url: string;
        if (preUrl) {
          if (preUrl.startsWith("rtmp://") || preUrl.startsWith("rtmps://")) {
            try {
              url = await invoke("get_playable_stream_url", { liveId }) as string;
            } catch {
              url = await invoke("get_live_stream_url_v2", { liveId }) as string;
            }
          } else {
            url = preUrl;
          }
        } else {
          try {
            url = await invoke("get_playable_stream_url", { liveId }) as string;
          } catch {
            url = await invoke("get_live_stream_url_v2", { liveId }) as string;
          }
        }

        if (cancelled) return;
        setStreamUrl(url);

        if (url.startsWith("rtmp://") || url.startsWith("rtmps://")) {
          if (tryOpenNativeAndroidPlayer(url)) {
            onClose();
            return;
          }
          setError("获取到的流地址为 RTMP 协议，当前环境无法直接播放");
          return;
        }

        if (isAndroid && isLive) {
          setUseCanvas(true);
        }
      } catch (e) {
        console.error(e);
        if (!cancelled) {
          setError("获取播放地址失败");
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    };
    loadStream();
    return () => {
      cancelled = true;
    };
  }, [liveId, preUrl, isLive, onClose, tryOpenNativeAndroidPlayer]);

  // Initialize player
  useEffect(() => {
    if (!streamUrl || !videoRef.current) return;

    const video = videoRef.current;
    let cancelled = false;
    retryCountRef.current = 0;

    // 尝试在错误后恢复，最多重试3次
    const retryPlay = () => {
      if (cancelled || retryCountRef.current >= 3) return;
      retryCountRef.current++;
      console.log(`重试播放 (${retryCountRef.current}/3)...`);
      setTimeout(() => {
        if (!cancelled && video) {
          if (hlsRef.current) {
            hlsRef.current.destroy();
            hlsRef.current = null;
          }
          video.pause();
          video.removeAttribute("src");
          video.load();
          initPlayer();
        }
      }, 2000 * retryCountRef.current);
    };

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
                debug: false,
              });
              hls.loadSource(streamUrl);
              hls.attachMedia(video);
              hls.on(Hls.Events.MANIFEST_PARSED, () => {
                if (!cancelled) {
                  video.play().then(() => {
                    if (useCanvas) startCanvasLoop();
                    retryCountRef.current = 0;
                  }).catch((e) => {
                    console.error("play failed:", e);
                    // 尝试恢复
                    setTimeout(() => {
                      if (!cancelled) video.play().catch(console.error);
                    }, 1000);
                  });
                }
              });
              hls.on(Hls.Events.ERROR, (_, data) => {
                if (data.fatal) {
                  if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
                    console.log("HLS network error, reloading...");
                    hls.startLoad();
                  } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
                    console.log("HLS media error, recovering...");
                    hls.recoverMediaError();
                  } else {
                    console.error("HLS fatal error:", data.type, data.details);
                    // 尝试重试
                    retryPlay();
                  }
                }
              });
              hlsRef.current = hls;
            } else {
              // hls.js 不支持，直接设置 src（Android 原生可能支持 HLS）
              video.src = streamUrl;
              video.play().then(() => {
                if (useCanvas) startCanvasLoop();
              }).catch((e) => {
                console.error("direct play failed:", e);
                setError("播放失败");
              });
            }
          } else {
            // 浏览器原生支持 HLS（如 Safari/macOS）
            video.src = streamUrl;
            video.play().then(() => {
              if (useCanvas) startCanvasLoop();
            }).catch((e) => {
              console.error("native HLS play failed:", e);
              setError("播放失败");
            });
          }
        } else {
          video.src = streamUrl;
          video.play().then(() => {
            if (useCanvas) startCanvasLoop();
          }).catch((e) => {
            console.error("direct play failed:", e);
            setError("播放失败");
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

  const handleVideoTouchStart = useCallback((e: React.TouchEvent) => {
    const touch = e.touches[0];
    if (!touch) return;
    swipeStartRef.current = { x: touch.clientX, y: touch.clientY };
  }, []);

  const handleVideoTouchEnd = useCallback((e: React.TouchEvent) => {
    const start = swipeStartRef.current;
    const touch = e.changedTouches[0];
    swipeStartRef.current = null;
    if (!start || !touch || loading || error || !onSwitch) return;

    const dx = touch.clientX - start.x;
    const dy = touch.clientY - start.y;
    if (Math.abs(dy) <= Math.abs(dx) || Math.abs(dy) < 72) return;

    if (switchingRef.current) return;
    switchingRef.current = true;
    if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
    setShowControls(false);

    const direction = dy < 0 ? "next" : "prev";
    const outTransform = direction === "next" ? "translateY(-100%)" : "translateY(100%)";
    const inTransform = direction === "next" ? "translateY(100%)" : "translateY(-100%)";

    setSwitchStyle({
      transform: outTransform,
      opacity: 1,
      transition: "transform 220ms ease-in-out, opacity 0ms linear",
    });

    window.setTimeout(async () => {
      try {
        await onSwitch(direction);
      } finally {
        setSwitchStyle({
          transform: inTransform,
          opacity: 1,
          transition: "transform 0ms linear, opacity 0ms linear",
        });
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            setSwitchStyle({
              transform: "translateY(0)",
              opacity: 1,
              transition: "transform 220ms ease-in-out, opacity 0ms linear",
            });
            window.setTimeout(() => {
              switchingRef.current = false;
            }, 230);
          });
        });
      }
    }, 220);
  }, [error, loading, onSwitch]);

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
      <button
        onClick={(e) => { e.stopPropagation(); onClose(); }}
        style={{
          position: "absolute",
          top: "24px",
          left: "16px",
          zIndex: 30,
          background: "rgba(0,0,0,0.35)",
          border: "none",
          color: "white",
          cursor: "pointer",
          padding: "6px",
          borderRadius: "999px",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
        aria-label="关闭播放"
      >
        <DismissRegular style={{ fontSize: "22px" }} />
      </button>

      {/* Top bar */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          padding: "24px 16px 10px 56px",
          flexShrink: 0,
          opacity: 1,
          pointerEvents: "auto",
          background: "linear-gradient(to bottom, rgba(0,0,0,0.55) 0%, rgba(0,0,0,0) 100%)",
        }}
      >
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

      {/* Video area */}
      <div
        onClick={handleVideoClick}
        onTouchStart={handleVideoTouchStart}
        onTouchEnd={handleVideoTouchEnd}
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          overflow: "hidden",
          position: "relative",
          minHeight: 0,
          transform: switchStyle.transform,
          opacity: switchStyle.opacity,
          transition: switchStyle.transition,
        }}
      >
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

        {error && (
          <div style={{ textAlign: "center", color: "#ff6b6b", padding: "20px", zIndex: 5 }}>
            <div style={{ fontSize: "48px", marginBottom: "12px" }}>!</div>
            <div style={{ marginBottom: "16px" }}>{error}</div>
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
        )}

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

        <video
          ref={videoRef}
          playsInline
          // 不设置 crossOrigin="anonymous" 以避免与某些 HLS 流发生 CORS 冲突
          // 某些流服务器可能不返回 CORS 头部，导致播放失败
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
              opacity: showControls ? 0.9 : 0,
              transition: "opacity 0.25s ease",
              cursor: "pointer",
              zIndex: 10,
              pointerEvents: showControls ? "auto" : "none",
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

      {/* Bottom controls */}
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
