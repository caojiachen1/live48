import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import {
  DismissRegular,
  PlayRegular,
  PauseRegular,
  PictureInPictureExitRegular,
  SpeakerMuteRegular,
} from "@fluentui/react-icons";

// Warm the demuxer chunks while the window is still booting, so initPlayer does not pay
// for a dynamic import after the config lands.
import("hls.js").catch(() => {});
import("mpegts.js").catch(() => {});

interface MiniConfig {
  title: string;
  nickname: string;
  stream_url: string;
  is_live: boolean;
}

function MiniPlayer() {
  const videoRef = useRef<HTMLVideoElement>(null);
  const hlsRef = useRef<any>(null);
  const mpegtsRef = useRef<any>(null);
  const retryCountRef = useRef(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [isPlaying, setIsPlaying] = useState(false);
  const [config, setConfig] = useState<MiniConfig | null>(null);
  const [resolvedUrl, setResolvedUrl] = useState("");
  const vlcUsedRef = useRef(false);
  const [isMuted, setIsMuted] = useState(false);

  // An autoplay that carries audio can still be rejected by the WebView2 autoplay policy.
  // Mute and retry so the picture shows anyway, and let the user bring sound back.
  const tryPlay = useCallback((v: HTMLVideoElement) => {
    return v.play().catch(() => {
      v.muted = true;
      setIsMuted(true);
      return v.play().catch(() => {});
    });
  }, []);

  // Pull the config the backend stored when it created this window
  useEffect(() => {
    invoke<MiniConfig | null>("get_mini_player_config")
      .then((cfg) => {
        if (cfg) {
          setConfig(cfg);
        } else {
          setError("未获取到播放配置");
          setLoading(false);
        }
      })
      .catch((e) => {
        setError("获取配置失败: " + String(e));
        setLoading(false);
      });
  }, []);

  // Resolve stream URL (handle RTMP via VLC transcoding)
  useEffect(() => {
    if (!config) return;
    const url = config.stream_url;
    if (!url) {
      setError("未提供播放地址");
      setLoading(false);
      return;
    }
    if (url.startsWith("rtmp://") || url.startsWith("rtmps://")) {
      invoke<string>("vlc_play", { url })
        .then((localUrl) => {
          vlcUsedRef.current = true;
          setResolvedUrl(localUrl);
        })
        .catch(() => {
          setError("RTMP 流无法直接播放，VLC 转码失败");
          setLoading(false);
        });
    } else {
      setResolvedUrl(url);
    }
    return () => {
      if (vlcUsedRef.current) {
        vlcUsedRef.current = false;
        invoke("vlc_stop").catch(() => {});
      }
    };
  }, [config]);

  // Initialize video player
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !resolvedUrl) return;
    let cancelled = false;
    retryCountRef.current = 0;

    const retryPlay = () => {
      if (cancelled || retryCountRef.current >= 3) return;
      retryCountRef.current++;
      setTimeout(() => {
        if (!cancelled && video) {
          if (hlsRef.current) { hlsRef.current.destroy(); hlsRef.current = null; }
          if (mpegtsRef.current) { mpegtsRef.current.destroy(); mpegtsRef.current = null; }
          video.pause();
          video.removeAttribute("src");
          video.load();
          initPlayer();
        }
      }, 2000 * retryCountRef.current);
    };

    const initPlayer = async () => {
      try {
        if (resolvedUrl.includes(".m3u8")) {
          const Hls = (await import("hls.js")).default;
          if (Hls.isSupported()) {
            const hls = new Hls({
              enableWorker: true,
              lowLatencyMode: false,
              maxBufferLength: 30,
              maxMaxBufferLength: 60,
              startLevel: -1,
              capLevelToPlayerSize: true,
            });
            hls.loadSource(resolvedUrl);
            hls.attachMedia(video);
            hls.on(Hls.Events.MANIFEST_PARSED, () => {
              if (!cancelled) {
                tryPlay(video).then(() => { retryCountRef.current = 0; }).catch(() => {
                  setTimeout(() => { if (!cancelled) tryPlay(video); }, 1000);
                });
              }
            });
            hls.on(Hls.Events.ERROR, (_: any, data: any) => {
              if (data.fatal) {
                if (data.type === Hls.ErrorTypes.NETWORK_ERROR) hls.startLoad();
                else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) hls.recoverMediaError();
                else retryPlay();
              }
            });
            hlsRef.current = hls;
          } else if (video.canPlayType("application/vnd.apple.mpegurl")) {
            video.src = resolvedUrl;
            tryPlay(video);
          } else {
            setError("当前环境不支持播放该格式");
          }
        } else if (resolvedUrl.endsWith(".mpegts") || resolvedUrl.endsWith(".ts")) {
          const Mpegts = (await import("mpegts.js")).default;
          if (Mpegts.isSupported()) {
            const player = Mpegts.createPlayer(
              { type: "mpegts", url: resolvedUrl, isLive: true, hasVideo: true, hasAudio: true },
              { lazyLoad: false, liveBufferLatencyChasing: true, liveBufferLatencyMaxLatency: 2.5, liveBufferLatencyMinRemain: 0.5, deferLoadAfterSourceOpen: false }
            );
            player.attachMediaElement(video);
            player.load();
            tryPlay(video);
            let tsRetries = 0;
            player.on(Mpegts.Events.ERROR, () => {
              if (cancelled || tsRetries >= 5) return;
              tsRetries++;
              const delay = Math.min(1000 * Math.pow(2, tsRetries - 1), 8000);
              setTimeout(() => {
                if (cancelled) return;
                try { player.unload(); player.load(); tryPlay(video); } catch {}
              }, delay);
            });
            mpegtsRef.current = player;
          } else {
            video.src = resolvedUrl;
            tryPlay(video);
          }
        } else {
          video.src = resolvedUrl;
          tryPlay(video);
        }
      } catch {
        if (!cancelled) setError("播放初始化失败");
      }
    };

    initPlayer();

    return () => {
      cancelled = true;
      if (hlsRef.current) { hlsRef.current.destroy(); hlsRef.current = null; }
      if (mpegtsRef.current) { mpegtsRef.current.destroy(); mpegtsRef.current = null; }
      try { video.pause(); } catch {}
      video.removeAttribute("src");
      video.load();
    };
  }, [resolvedUrl, tryPlay]);

  // Video event listeners
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    const onPlay = () => { setIsPlaying(true); setLoading(false); };
    const onPause = () => setIsPlaying(false);
    const onPlaying = () => setLoading(false);
    const onError = () => setError("播放出错");
    const onVolumeChange = () => setIsMuted(video.muted);
    video.addEventListener("play", onPlay);
    video.addEventListener("pause", onPause);
    video.addEventListener("playing", onPlaying);
    video.addEventListener("error", onError);
    video.addEventListener("volumechange", onVolumeChange);
    return () => {
      video.removeEventListener("play", onPlay);
      video.removeEventListener("pause", onPause);
      video.removeEventListener("playing", onPlaying);
      video.removeEventListener("error", onError);
      video.removeEventListener("volumechange", onVolumeChange);
    };
  }, []);

  const togglePlay = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) tryPlay(video);
    else video.pause();
  }, [tryPlay]);

  const handleDragStart = useCallback(() => {
    getCurrentWindow().startDragging();
  }, []);

  // During a fast native resize the webview keeps presenting the previous frame stretched
  // into the new bounds, which makes the bottom-anchored controls strobe visibly. Drop them
  // for the duration of the drag and bring them back once the size settles.
  const [resizing, setResizing] = useState(false);
  const resizingRef = useRef(false);
  const resizeSettleRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const armResizeSettle = useCallback((delayMs: number) => {
    if (resizeSettleRef.current) clearTimeout(resizeSettleRef.current);
    resizeSettleRef.current = setTimeout(() => {
      resizingRef.current = false;
      setResizing(false);
    }, delayMs);
  }, []);

  useEffect(() => {
    const unlisten = getCurrentWindow().onResized(() => {
      if (!resizingRef.current) return;
      armResizeSettle(160);
    });
    return () => {
      if (resizeSettleRef.current) clearTimeout(resizeSettleRef.current);
      unlisten.then((f) => f());
    };
  }, [armResizeSettle]);

  // A borderless window has no native resize border above its client area, so the corner
  // handle must ask the OS to run the top-left resize loop itself.
  const handleResizeStart = useCallback(() => {
    resizingRef.current = true;
    setResizing(true);
    armResizeSettle(400); // 万一这次拖拽根本没改变尺寸，靠它把控件放回来
    getCurrentWindow().startResizeDragging("NorthWest");
  }, [armResizeSettle]);

  const unmute = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    video.muted = false;
    tryPlay(video);
  }, [tryPlay]);

  // show_main_window + mini-player-closed are handled once by onCloseRequested below,
  // which also fires for this close().
  const handleClose = useCallback(() => {
    getCurrentWindow().close();
  }, []);

  useEffect(() => {
    const unlisten = getCurrentWindow().onCloseRequested(async () => {
      try { await invoke("show_main_window"); } catch {}
      await emit("mini-player-closed").catch(() => {});
    });
    return () => { unlisten.then(f => f()); };
  }, []);

  const title = config?.title || "无标题";
  const nickname = config?.nickname || "";

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        backgroundColor: "#000",
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
        userSelect: "none",
        WebkitUserSelect: "none",
      }}
    >
      <div
        onMouseDown={(e) => { e.stopPropagation(); handleResizeStart(); }}
        style={{
          position: "absolute", top: 0, left: 0, width: "30px", height: "30px",
          cursor: "nwse-resize", zIndex: 30,
          display: "flex", alignItems: "flex-start", justifyContent: "flex-start", padding: "5px",
        }}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" style={{ opacity: 0.5 }}>
          <line x1="1" y1="9" x2="9" y2="1" stroke="white" strokeWidth="1.5" />
          <line x1="1" y1="5" x2="5" y2="1" stroke="white" strokeWidth="1.5" />
        </svg>
      </div>

      <button
        onClick={handleClose}
        style={{
          position: "absolute", top: "4px", right: "4px", zIndex: 30,
          background: "rgba(0,0,0,0.5)", border: "none", color: "white",
          cursor: "pointer", padding: "4px", borderRadius: "50%",
          display: resizing ? "none" : "flex", alignItems: "center", justifyContent: "center",
        }}
        aria-label="关闭播放"
      >
        <DismissRegular style={{ fontSize: "18px" }} />
      </button>

      <div
        onMouseDown={handleDragStart}
        style={{
          flex: 1, display: "flex", alignItems: "center", justifyContent: "center",
          overflow: "hidden", position: "relative", minHeight: 0, cursor: "move",
        }}
      >
        {loading && (
          <div style={{
            position: "absolute", inset: 0, zIndex: 5, background: "#000",
            display: "flex", flexDirection: "column",
            alignItems: "center", justifyContent: "center",
            color: "rgba(255,255,255,0.7)",
          }}>
            <div style={{
              width: "36px", height: "36px",
              border: "3px solid rgba(255,255,255,0.2)", borderTop: "3px solid #bb86fc",
              borderRadius: "50%", animation: "mini-spin 0.8s linear infinite",
              margin: "0 auto 12px auto",
            }} />
            <div style={{ fontSize: "12px" }}>{config ? "加载中..." : "等待配置..."}</div>
          </div>
        )}
        {error && (
          <div style={{
            position: "absolute", inset: 0, zIndex: 6, background: "#000",
            display: "flex", alignItems: "center", justifyContent: "center",
            color: "#ff6b6b", padding: "16px", fontSize: "12px", textAlign: "center",
          }}>
            <div>{error}</div>
          </div>
        )}
        <video
          ref={videoRef}
          playsInline
          style={{
            // 用 auto + max 时，视频盒本身会在"按宽夹"和"按高夹"之间来回取整，
            // 缩放窗口时每帧跳一两像素；改成铺满容器，交给 object-fit 做信箱。
            width: "100%", height: "100%", objectFit: "contain",
            backgroundColor: "#000",
          }}
        />
      </div>

      {!loading && !error && !resizing && (
        <div
          onClick={(e) => e.stopPropagation()}
          onMouseDown={(e) => e.stopPropagation()}
          style={{
            position: "absolute", bottom: 0, left: 0, right: 0,
            display: "flex", alignItems: "center", gap: "4px",
            padding: "4px 6px",
            background: "linear-gradient(to top, rgba(0,0,0,0.94) 0%, rgba(0,0,0,0.72) 100%)",
            zIndex: 20,
          }}
        >
          <div style={{
            flex: 1, minWidth: 0, color: "rgba(255,255,255,0.85)", fontSize: "11px",
            whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis",
          }}>
            {title}
          </div>
          {nickname && (
            <div style={{
              color: "rgba(255,255,255,0.45)", fontSize: "10px",
              whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", maxWidth: "60px",
            }}>
              {nickname}
            </div>
          )}
          {isMuted && (
            <button
              onClick={(e) => { e.stopPropagation(); unmute(); }}
              style={{
                background: "none", border: "none", color: "#bb86fc",
                cursor: "pointer", padding: "2px",
                display: "flex", alignItems: "center", justifyContent: "center",
              }}
              aria-label="开启声音"
              title="点击开启声音"
            >
              <SpeakerMuteRegular style={{ fontSize: "16px" }} />
            </button>
          )}
          <button
            onClick={(e) => { e.stopPropagation(); togglePlay(); }}
            style={{
              background: "none", border: "none", color: "white",
              cursor: "pointer", padding: "2px",
              display: "flex", alignItems: "center", justifyContent: "center",
            }}
            aria-label={isPlaying ? "暂停" : "播放"}
          >
            {isPlaying ? <PauseRegular style={{ fontSize: "16px" }} /> : <PlayRegular style={{ fontSize: "16px" }} />}
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); handleClose(); }}
            style={{
              background: "none", border: "none", color: "white",
              cursor: "pointer", padding: "2px",
              display: "flex", alignItems: "center", justifyContent: "center",
            }}
            aria-label="全屏播放"
            title="全屏播放"
          >
            <PictureInPictureExitRegular style={{ fontSize: "16px" }} />
          </button>
        </div>
      )}

      <style>{`
        @keyframes mini-spin {
          0% { transform: rotate(0deg); }
          100% { transform: rotate(360deg); }
        }
        body { margin: 0; padding: 0; overflow: hidden; background: #000; }
      `}</style>
    </div>
  );
}

export default MiniPlayer;
