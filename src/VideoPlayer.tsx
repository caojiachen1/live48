import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DismissRegular, PlayRegular, PauseRegular, ArrowClockwiseRegular, ListRegular } from "@fluentui/react-icons";

interface PlaylistItem {
  liveId: string;
  title?: string;
  nickname?: string;
  coverPath?: string;
  userInfo?: { nickname: string };
}

interface VideoPlayerProps {
  liveId: string;
  title: string;
  nickname: string;
  streamUrl?: string;
  isLive?: boolean;
  canSwitchPrev?: boolean;
  canSwitchNext?: boolean;
  onSwitch?: (direction: "prev" | "next") => string | void | Promise<string | void>;
  playlist?: PlaylistItem[];
  currentIndex?: number;
  onSelect?: (index: number) => void;
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

function VideoPlayer({ liveId, title, nickname, streamUrl: preUrl, isLive, canSwitchPrev, canSwitchNext, onSwitch, playlist, currentIndex, onSelect, onClose }: VideoPlayerProps) {
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
  const [retryKey, setRetryKey] = useState(0);
  const hlsRef = useRef<any>(null);
  const mpegtsRef = useRef<any>(null);
  const vlcUsedRef = useRef(false);

  // Timeline state
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [buffered, setBuffered] = useState(0);
  const [isSeeking, setIsSeeking] = useState(false);
  const [showControls, setShowControls] = useState(false);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const swipeStartRef = useRef<{ x: number; y: number } | null>(null);
  const switchingRef = useRef(false);
  const [boundaryToast, setBoundaryToast] = useState<string | null>(null);
  const [showPlaylist, setShowPlaylist] = useState(false);
  const playlistRef = useRef<HTMLDivElement>(null);
  const boundaryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
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
      if (boundaryTimerRef.current) clearTimeout(boundaryTimerRef.current);
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

        // 如果拿到的仍然是 RTMP 流，尝试 VLC 转码
        if (url.startsWith("rtmp://") || url.startsWith("rtmps://")) {
          try {
            const localUrl = await invoke("vlc_play", { url }) as string;
            if (cancelled) return;
            url = localUrl;
            vlcUsedRef.current = true;
          } catch {
            if (cancelled) return;
            if (tryOpenNativeAndroidPlayer(url)) {
              onClose();
              return;
            }
            setError("获取到的流地址为 RTMP 协议，当前环境无法直接播放");
            return;
          }
        }

        setStreamUrl(url);

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
      if (vlcUsedRef.current) {
        vlcUsedRef.current = false;
        invoke("vlc_stop").catch(() => {});
      }
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
            }).catch((e: any) => {
              if (e.name !== "AbortError") {
                console.error("native HLS play failed:", e);
                setError("播放失败");
              }
            });
          }
        } else if (streamUrl.endsWith(".mpegts") || streamUrl.endsWith(".ts")) {
          // MPEG-TS 流，浏览器不支持原生播放，使用 mpegts.js 解码
          const Mpegts = (await import("mpegts.js")).default;
          if (Mpegts.isSupported()) {
            const player = Mpegts.createPlayer({
              type: "mpegts",
              url: streamUrl,
              isLive: true,
              hasVideo: true,
              hasAudio: true,
            }, {
              enableStashBuffer: false,
              liveBufferLatencyChasing: true,
              liveBufferLatencyMaxLatency: 1.5,
              liveBufferLatencyMinRemain: 0.2,
              lazyLoadMaxDuration: 0.5,
              deferLoadAfterSourceOpen: false,
            });
            player.attachMediaElement(video);
            player.load();
            const playPromise = player.play();
            if (playPromise !== undefined) {
              playPromise.catch((e: any) => {
                if (e.name !== "AbortError") {
                  console.error("mpegts play failed:", e);
                }
              });
            }

            // 直播流断流自动重连（最多 5 次，指数退避）
            let tsRetries = 0;
            player.on(Mpegts.Events.ERROR, () => {
              if (cancelled || tsRetries >= 5) return;
              tsRetries++;
              const delay = Math.min(1000 * Math.pow(2, tsRetries - 1), 8000);
              console.log(`mpegts error, retry ${tsRetries}/5 in ${delay}ms...`);
              setTimeout(() => {
                if (cancelled) return;
                try { player.unload(); player.load(); player.play()?.catch(() => {}); } catch {}
              }, delay);
            });

            mpegtsRef.current = player;
          } else {
            // mpegts.js 不支持，回退直接播放
            video.src = streamUrl;
            video.play().then(() => {
              if (useCanvas) startCanvasLoop();
            }).catch((e: any) => {
              if (e.name !== "AbortError") {
                console.error("direct play failed:", e);
                setError("播放失败");
              }
            });
          }
        } else {
          video.src = streamUrl;
          video.play().then(() => {
            if (useCanvas) startCanvasLoop();
          }).catch((e: any) => {
            if (e.name !== "AbortError") {
              console.error("direct play failed:", e);
              setError("播放失败");
            }
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
      if (mpegtsRef.current) {
        mpegtsRef.current.destroy();
        mpegtsRef.current = null;
      }
      // play() promise 可能未完成，pause() 触发 AbortError — 安全忽略
      try { video.pause(); } catch {}
      video.removeAttribute("src");
      video.load();
    };
  }, [streamUrl, useCanvas, startCanvasLoop, retryKey]);

  // Play/Pause toggle
  const togglePlay = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) {
      video.play().catch((e: any) => {
        if (e.name !== "AbortError") console.error(e);
      });
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

  const showBoundaryToast = useCallback((msg: string) => {
    setBoundaryToast(msg);
    if (boundaryTimerRef.current) clearTimeout(boundaryTimerRef.current);
    boundaryTimerRef.current = setTimeout(() => setBoundaryToast(null), 2000);
  }, []);

  const performSwitch = useCallback(async (direction: "prev" | "next") => {
    if (switchingRef.current || !onSwitch) return;
    if (direction === "prev" && !canSwitchPrev) { showBoundaryToast("已经是第一个了"); return; }
    if (direction === "next" && !canSwitchNext) { showBoundaryToast("已经是最后一个了"); return; }
    switchingRef.current = true;
    if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
    setShowControls(false);

    const outTransform = direction === "next" ? "translateY(-100%)" : "translateY(100%)";
    const inTransform = direction === "next" ? "translateY(100%)" : "translateY(-100%)";

    setSwitchStyle({
      transform: outTransform,
      opacity: 1,
      transition: "transform 220ms ease-in-out, opacity 0ms linear",
    });

    window.setTimeout(async () => {
      try {
        const msg = await onSwitch(direction);
        if (msg) showBoundaryToast(msg);
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
  }, [onSwitch, canSwitchPrev, canSwitchNext, showBoundaryToast]);

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

    performSwitch(dy < 0 ? "next" : "prev");
  }, [error, loading, onSwitch, performSwitch]);

  // Mouse wheel to switch prev/next video
  const handleWheel = useCallback((e: React.WheelEvent) => {
    if (loading || error || !onSwitch) return;
    if (Math.abs(e.deltaY) < 30) return;
    performSwitch(e.deltaY > 0 ? "next" : "prev");
  }, [error, loading, onSwitch, performSwitch]);

  // Keyboard up/down arrow to switch prev/next video
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        performSwitch(e.key === "ArrowUp" ? "prev" : "next");
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [performSwitch]);

  // Close overlay when clicking background (not video area)
  const handleOverlayClick = (e: React.MouseEvent) => {
    if (e.target === containerRef.current) onClose();
  };

  // Block body scroll while player is open
  useEffect(() => {
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => { document.body.style.overflow = prev; };
  }, []);

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
        overflow: "hidden",
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

      {isLive && (
        <button
          onClick={(e) => { e.stopPropagation(); setRetryKey(k => k + 1); }}
          style={{
            position: "absolute",
            top: "24px",
            right: playlist && playlist.length > 1 ? "56px" : "16px",
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
          aria-label="刷新直播流"
          title="刷新直播流"
        >
          <ArrowClockwiseRegular style={{ fontSize: "20px" }} />
        </button>
      )}

      {playlist && playlist.length > 1 && (
        <button
          onClick={(e) => { e.stopPropagation(); setShowPlaylist(v => !v); }}
          style={{
            position: "absolute",
            top: "24px",
            right: "16px",
            zIndex: 30,
            background: showPlaylist ? "rgba(187,134,252,0.5)" : "rgba(0,0,0,0.35)",
            border: "none",
            color: "white",
            cursor: "pointer",
            padding: "6px",
            borderRadius: "999px",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
          aria-label="播放列表"
          title="播放列表"
        >
          <ListRegular style={{ fontSize: "20px" }} />
        </button>
      )}

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
        onWheel={handleWheel}
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

      {/* Boundary toast */}
      {boundaryToast && (
        <div style={{
          position: "absolute",
          top: "50%",
          left: "50%",
          transform: "translate(-50%, -50%)",
          background: "rgba(0,0,0,0.7)",
          color: "rgba(255,255,255,0.9)",
          padding: "10px 24px",
          borderRadius: "8px",
          fontSize: "14px",
          zIndex: 50,
          pointerEvents: "none",
          whiteSpace: "nowrap",
        }}>
          {boundaryToast}
        </div>
      )}

      {/* Playlist panel */}
      {showPlaylist && playlist && playlist.length > 1 && (
        <>
          <div
            onClick={(e) => { e.stopPropagation(); setShowPlaylist(false); }}
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              right: 0,
              bottom: 0,
              zIndex: 25,
              background: "rgba(0,0,0,0.3)",
            }}
          />
          <div
            ref={playlistRef}
            onClick={(e) => e.stopPropagation()}
            style={{
              position: "absolute",
              top: 0,
              right: 0,
              bottom: 0,
              width: "280px",
              maxWidth: "80vw",
              zIndex: 30,
              background: "rgba(20,20,20,0.95)",
              borderLeft: "1px solid rgba(255,255,255,0.1)",
              display: "flex",
              flexDirection: "column",
              overflow: "hidden",
              animation: "playlist-slide-in 0.2s ease-out",
            }}
          >
            <div style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              padding: "16px",
              borderBottom: "1px solid rgba(255,255,255,0.1)",
              flexShrink: 0,
            }}>
              <span style={{ color: "rgba(255,255,255,0.9)", fontSize: "14px", fontWeight: 600 }}>
                播放列表 ({playlist.length})
              </span>
              <button
                onClick={() => setShowPlaylist(false)}
                style={{
                  background: "none",
                  border: "none",
                  color: "rgba(255,255,255,0.6)",
                  cursor: "pointer",
                  padding: "4px",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                }}
              >
                <DismissRegular style={{ fontSize: "16px" }} />
              </button>
            </div>
            <div style={{
              flex: 1,
              overflowY: "auto",
              overflowX: "hidden",
            }}>
              {playlist.map((item, idx) => {
                const isCurrent = idx === currentIndex;
                return (
                  <div
                    key={item.liveId}
                    onClick={() => {
                      if (!isCurrent && onSelect) {
                        onSelect(idx);
                        setShowPlaylist(false);
                      }
                    }}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: "10px",
                      padding: "10px 16px",
                      cursor: isCurrent ? "default" : "pointer",
                      background: isCurrent ? "rgba(187,134,252,0.15)" : "transparent",
                      borderLeft: isCurrent ? "3px solid #bb86fc" : "3px solid transparent",
                      transition: "background 0.15s ease",
                    }}
                    onMouseEnter={(e) => {
                      if (!isCurrent) (e.currentTarget as HTMLDivElement).style.background = "rgba(255,255,255,0.06)";
                    }}
                    onMouseLeave={(e) => {
                      if (!isCurrent) (e.currentTarget as HTMLDivElement).style.background = "transparent";
                    }}
                  >
                    {item.coverPath && (
                      <img
                        src={item.coverPath.startsWith("http") ? item.coverPath : `https://source3.48.cn${item.coverPath}`}
                        alt=""
                        style={{
                          width: "48px",
                          height: "48px",
                          objectFit: "cover",
                          borderRadius: "4px",
                          flexShrink: 0,
                        }}
                      />
                    )}
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{
                        color: isCurrent ? "#bb86fc" : "rgba(255,255,255,0.85)",
                        fontSize: "13px",
                        fontWeight: isCurrent ? 600 : 400,
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}>
                        {item.title || "无标题"}
                      </div>
                      <div style={{
                        color: "rgba(255,255,255,0.45)",
                        fontSize: "11px",
                        marginTop: "2px",
                        whiteSpace: "nowrap",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                      }}>
                        {item.nickname || item.userInfo?.nickname || ""}
                      </div>
                    </div>
                    {isCurrent && (
                      <div style={{
                        width: "8px",
                        height: "8px",
                        borderRadius: "50%",
                        backgroundColor: "#bb86fc",
                        flexShrink: 0,
                        animation: "player-live-pulse 1.5s ease-in-out infinite",
                      }} />
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        </>
      )}

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
        @keyframes playlist-slide-in {
          from { transform: translateX(100%); }
          to { transform: translateX(0); }
        }
      `}</style>
    </div>
  );
}

export default VideoPlayer;
