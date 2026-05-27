import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { 
  Button, 
  Card, 
  CardHeader, 
  CardPreview, 
  Text, 
  Spinner,
  Toast,
  ToastTitle,
  Toaster,
  useToastController,
  useId,
  Avatar,
  Input,
  Select,
  TabList,
  Tab,
  ProgressBar
} from "@fluentui/react-components";
import { PlayRegular, CopyRegular, ArrowClockwiseRegular, SearchRegular, ArrowDownloadRegular, DismissRegular, FilterRegular, SettingsRegular } from "@fluentui/react-icons";
import VideoPlayer from "./VideoPlayer";
import SettingsPanel from "./SettingsPanel";
import dayjs from "dayjs";
import "./App.android.css";

interface NativeAndroidPlayerBridge {
  isAvailable?: () => boolean;
  playStream: (url: string, title: string, nickname: string, isLive: boolean) => void;
  playPlaylist?: (playlistJson: string, startIndex: number, isLive: boolean) => void;
}

interface LiveItem {
  liveId: string;
  title: string;
  coverPath: string;
  liveType: number;
  inMicrophoneConnection?: boolean;
  ctime?: string;
  userInfo: {
    nickname: string;
    avatar: string;
  };
}

const getLiveTypeBadge = (type: number) => {
  let text = "未知";
  let color = "gray";
  if (type === 1) { text = "视频"; color = "purple"; }
  else if (type === 2) { text = "电台"; color = "#fecd00"; }
  else if (type === 3) { text = "连麦"; color = "blue"; }
  else if (type === 5) { text = "游戏"; color = "green"; }
  else if (type === 6) { text = "AI"; color = "teal"; }

  return { text, color };
};

function App() {
  const [activeTab, setActiveTab] = useState<"live" | "record">("live");
  const [lives, setLives] = useState<LiveItem[]>([]);
  const [records, setRecords] = useState<LiveItem[]>([]);
  const [recordNext, setRecordNext] = useState<string>("0");
  const [targetLoadTime, setTargetLoadTime] = useState<number>(0);
  const [loading, setLoading] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [downloadProgresses, setDownloadProgresses] = useState<Record<string, number>>({});
  const [filterGroup, setFilterGroup] = useState("全部");
  const [filterType, setFilterType] = useState("全部");
  const [filterOpen, setFilterOpen] = useState(false);
  const [pullDistance, setPullDistance] = useState(0);
  const [pullRefreshing, setPullRefreshing] = useState(false);
  const [swipeTranslation, setSwipeTranslation] = useState(0);
  
  // Settings state
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [useBuiltInVlc, setUseBuiltInVlc] = useState(() => {
    return localStorage.getItem("useBuiltInVlc") === "true";
  });
  const [vlcPlaying, setVlcPlaying] = useState(false);

  const handleUseBuiltInVlcChange = (value: boolean) => {
    setUseBuiltInVlc(value);
    localStorage.setItem("useBuiltInVlc", String(value));
  };

  const handleVlcStop = async () => {
    try {
      await invoke("vlc_stop");
      setVlcPlaying(false);
    } catch (e) {
      console.error(e);
    }
  };

  // Video player state (used on mobile/webview platforms)
  const [playingList, setPlayingList] = useState<LiveItem[]>([]);
  const [playingIndex, setPlayingIndex] = useState<number>(-1);
  const [playingUrl, setPlayingUrl] = useState<string>("");
  const rootRef = useRef<HTMLDivElement>(null);
  const gestureRef = useRef({
    startX: 0,
    startY: 0,
    tracking: false,
    tabSwitched: false,
    pulling: false,
  });

  const toasterId = useId("toaster");
  const { dispatchToast } = useToastController(toasterId);

  const fetchLiveInfo = async () => {
    try {
      const res: any = await invoke("get_live_list");
      if (res.content && res.content.liveList) {
        setLives(res.content.liveList);
      }
    } catch (e) {
      console.error(e);
      dispatchToast(<Toast><ToastTitle>获取直播列表失败</ToastTitle></Toast>, { intent: "error" });
    }
  };

  const fetchRecordInfo = async () => {
    try {
      const initialTargetTime = dayjs().subtract(1, 'day').startOf('day').valueOf();
      setTargetLoadTime(initialTargetTime);

      let currentNext = "0";
      let accumRecords: LiveItem[] = [];
      let done = false;

      while (!done) {
        const res: any = await invoke("get_record_list", { next: currentNext });
        if (res.content && res.content.liveList) {
          const newRecords: LiveItem[] = res.content.liveList;
          accumRecords = [...accumRecords, ...newRecords];
          currentNext = res.content.next || "0";

          if (newRecords.length > 0) {
            const lastRecord = newRecords[newRecords.length - 1];
            if (lastRecord && lastRecord.ctime && Number(lastRecord.ctime) < initialTargetTime) {
              done = true;
            }
          } else {
            done = true;
          }
        } else {
          done = true;
        }

        if (currentNext === "0") {
          done = true;
        }
      }

      setRecords(accumRecords);
      setRecordNext(currentNext);
    } catch (e) {
      console.error(e);
      dispatchToast(<Toast><ToastTitle>获取录播列表失败</ToastTitle></Toast>, { intent: "error" });
    }
  };

  const fetchLiveList = async (tab?: "live" | "record") => {
    const target = tab || activeTab;
    setLoading(true);
    try {
      if (target === "live") {
        await fetchLiveInfo();
      } else {
        await fetchRecordInfo();
      }
    } finally {
      setLoading(false);
    }
  };

  const loadMoreRecords = async () => {
    if (activeTab !== "record" || recordNext === "0" || loading) return;
    setLoading(true);
    try {
      const nextTargetTime = dayjs(targetLoadTime).subtract(1, 'day').startOf('day').valueOf();
      setTargetLoadTime(nextTargetTime);

      let currentNext = recordNext;
      let accumRecords = [...records];
      let done = false;

      while (!done) {
        const res: any = await invoke("get_record_list", { next: currentNext });
        if (res.content && res.content.liveList) {
          const newRecords: LiveItem[] = res.content.liveList;
          accumRecords = [...accumRecords, ...newRecords];
          currentNext = res.content.next || "0";

          if (newRecords.length > 0) {
            const lastRecord = newRecords[newRecords.length - 1];
            if (lastRecord && lastRecord.ctime && Number(lastRecord.ctime) < nextTargetTime) {
              done = true;
            }
          } else {
            done = true;
          }
        } else {
          done = true;
        }

        if (currentNext === "0") {
          done = true;
        }
      }

      setRecords(accumRecords);
      setRecordNext(currentNext);
    } catch (e) {
      console.error(e);
      dispatchToast(
        <Toast><ToastTitle>加载更多失败</ToastTitle></Toast>,
        { intent: "error" }
      );
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    // 首次加载时缓存双端数据
    const initFetch = async () => {
      setLoading(true);
      await Promise.all([fetchLiveInfo(), fetchRecordInfo()]);
      setLoading(false);
    };
    initFetch();
  }, []);

  useEffect(() => {
    const unlisten = listen("download-status", (event: any) => {
      const payload = event.payload;
      if (payload.status === "start") {
        setDownloadProgresses(prev => ({ ...prev, [payload.live_id]: 0 }));
        dispatchToast(<Toast><ToastTitle>正在下载：{payload.message}</ToastTitle></Toast>, { intent: "info" });
      } else if (payload.status === "progress") {
        setDownloadProgresses(prev => ({ ...prev, [payload.live_id]: payload.progress }));
      } else if (payload.status === "success") {
        setDownloadProgresses(prev => {
          const next = { ...prev };
          delete next[payload.live_id];
          return next;
        });
        dispatchToast(<Toast><ToastTitle>下载完成：{payload.message}</ToastTitle></Toast>, { intent: "success" });
      } else if (payload.status === "error") {
        setDownloadProgresses(prev => {
          const next = { ...prev };
          delete next[payload.live_id];
          return next;
        });
        dispatchToast(<Toast><ToastTitle>下载失败：{payload.message}</ToastTitle></Toast>, { intent: "error" });
      } else if (payload.status === "cancelled") {
        setDownloadProgresses(prev => {
          const next = { ...prev };
          delete next[payload.live_id];
          return next;
        });
        dispatchToast(<Toast><ToastTitle>下载已取消：{payload.message}</ToastTitle></Toast>, { intent: "warning" });
      }
    });

    return () => {
      unlisten.then(f => f());
    };
  }, []);

  // Detect if running in a mobile WebView (Android)
  const isAndroid = /Android/i.test(navigator.userAgent);
  const isMobile = /Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini/i.test(navigator.userAgent) 
    || window.innerWidth <= 768;

  const filterList = (list: LiveItem[], isRecordTab: boolean) => {
    return list.filter(item => {
      if (isRecordTab && targetLoadTime > 0 && item.ctime) {
        if (Number(item.ctime) < targetLoadTime) {
          return false;
        }
      }

      const matchSearch = item.userInfo.nickname.toLowerCase().includes(searchQuery.toLowerCase()) || 
        (item.title && item.title.toLowerCase().includes(searchQuery.toLowerCase()));

      const matchGroup = filterGroup === "全部" || item.userInfo.nickname.toUpperCase().startsWith(filterGroup);

      let matchType = true;
      if (filterType !== "全部") {
        if (filterType === "连麦") {
          matchType = !!item.inMicrophoneConnection;
        } else {
          const badge = getLiveTypeBadge(item.liveType);
          matchType = badge.text === filterType;
        }
      }

      return matchSearch && matchGroup && matchType;
    });
  };

  const filteredLiveItems = filterList(lives, false);
  const filteredRecordItems = filterList(records, true);
  const filteredItems = activeTab === "live" ? filteredLiveItems : filteredRecordItems;

  const shouldIgnoreGestureTarget = (target: EventTarget | null) => {
    if (!(target instanceof HTMLElement)) return false;
    return Boolean(
      target.closest(
        "button,input,select,textarea,a,[role='button'],[role='tab'],.fui-Button,.fui-Input,.fui-Select,.fui-Tab"
      )
    );
  };

  const handleListTouchStart = (e: React.TouchEvent) => {
    if (!isMobile || playingIndex >= 0 || shouldIgnoreGestureTarget(e.target)) return;
    const touch = e.touches[0];
    if (!touch) return;
    gestureRef.current.startX = touch.clientX;
    gestureRef.current.startY = touch.clientY;
    gestureRef.current.tracking = true;
    gestureRef.current.tabSwitched = false;
    gestureRef.current.pulling = false;
    setSwipeTranslation(0);
  };

  const handleListTouchMove = (e: React.TouchEvent | TouchEvent) => {
    if (!isMobile || playingIndex >= 0 || !gestureRef.current.tracking) return;
    const touch = e.touches[0];
    if (!touch) return;

    const dx = touch.clientX - gestureRef.current.startX;
    const dy = touch.clientY - gestureRef.current.startY;
    const absDx = Math.abs(dx);
    const absDy = Math.abs(dy);

    if (absDx > absDy && absDx > 10) {
      if (activeTab === "live" && dx > 0) return; // Cant swipe right from live
      if (activeTab === "record" && dx < 0) return; // Cant swipe left from record
      
      setSwipeTranslation(dx);
      if (!("cancelable" in e) || e.cancelable) {
        e.preventDefault();
      }
      return;
    }

    if (window.scrollY <= 0 && dy > 0 && absDy > absDx) {
      gestureRef.current.pulling = true;
      const nextDistance = Math.min(96, dy * 0.45);
      setPullDistance(nextDistance);
      if (!("cancelable" in e) || e.cancelable) {
        e.preventDefault();
      }
    }
  };

  const handleListTouchEnd = async () => {
    if (playingIndex >= 0) return;
    const shouldRefresh = gestureRef.current.pulling && pullDistance >= 56 && !loading;
    gestureRef.current.tracking = false;
    gestureRef.current.pulling = false;
    
    if (swipeTranslation !== 0) {
      // 极低的触发阈值，只要有超过20像素的水平滑动意图就会直接切换
      const threshold = 10;
      if (activeTab === "live" && swipeTranslation < -threshold) {
        setActiveTab("record");
      } else if (activeTab === "record" && swipeTranslation > threshold) {
        setActiveTab("live");
      }
      setSwipeTranslation(0);
    }

    if (shouldRefresh) {
      setPullDistance(56); // 保持在下拉的位置转圈
      setPullRefreshing(true);
      try {
        await fetchLiveList();
      } finally {
        setPullRefreshing(false);
        setPullDistance(0);
      }
    } else {
      setPullDistance(0);
    }
  };

  const switchWebPlayback = async (direction: "prev" | "next") => {
    if (playingIndex < 0 || playingList.length === 0) return;
    const nextIndex = direction === "next" ? playingIndex + 1 : playingIndex - 1;
    if (nextIndex < 0 || nextIndex >= playingList.length) return;

    const nextItem = playingList[nextIndex];
    try {
      const nextUrl: string = await invoke("get_live_stream_url", { liveId: nextItem.liveId });
      setPlayingIndex(nextIndex);
      setPlayingUrl(nextUrl);
    } catch (e) {
      console.error(e);
      dispatchToast(
        <Toast><ToastTitle>切换播放失败</ToastTitle></Toast>,
        { intent: "error" }
      );
    }
  };

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const handler = (event: TouchEvent) => {
      handleListTouchMove(event);
    };
    root.addEventListener("touchmove", handler, { passive: false });
    return () => {
      root.removeEventListener("touchmove", handler);
    };
  }, [handleListTouchMove]);

  const handlePlayOrCopy = async (liveId: string, action: "play" | "copy" | "download", item?: LiveItem) => {
    try {
      const url: string = await invoke("get_live_stream_url", { liveId });
      
      if (action === "copy") {
        await writeText(url);
        dispatchToast(
          <Toast><ToastTitle>成功复制推流/录播地址</ToastTitle></Toast>,
          { intent: "success" }
        );
      } else if (action === "download") {
        const filePath = await save({
          filters: [{ name: 'Video File', extensions: ['ts', 'mp4', 'flv'] }],
          defaultPath: `Live_${liveId}.ts`
        });
        if (filePath) {
          await invoke("download_video", { url, savePath: filePath, liveId });
        }
      } else if (action === "play") {
        const startIndex = filteredItems.findIndex(i => i.liveId === liveId);
        const orderedItems = startIndex >= 0 ? filteredItems : (item ? [item] : []);

        // Android: all streams (live + record) use native player first
        if (isAndroid && item) {
          const bridge = (window as Window & { Live48NativePlayer?: NativeAndroidPlayerBridge }).Live48NativePlayer;
          if (bridge && typeof bridge.playStream === "function") {
            if (!bridge.isAvailable || bridge.isAvailable()) {
              if (bridge.playPlaylist && orderedItems.length > 0) {
                const playlistPayload = orderedItems.map((entry) => ({
                  liveId: entry.liveId,
                  title: entry.title || "无标题",
                  nickname: entry.userInfo.nickname,
                  streamUrl: entry.liveId === liveId ? url : "",
                }));
                bridge.playPlaylist(JSON.stringify(playlistPayload), Math.max(0, startIndex), activeTab === "live");
              } else {
                bridge.playStream(url, item.title || "无标题", item.userInfo.nickname, activeTab === "live");
              }
              return;
            }
          }
        }

        // Non-Android mobile fallback: in-app web player
        if (isMobile && item) {
          setPlayingList(orderedItems.length > 0 ? orderedItems : [item]);
          setPlayingIndex(Math.max(0, startIndex));
          setPlayingUrl(url);
          } else {
            // Desktop
            if (useBuiltInVlc) {
              // 仅 RTMP 需要 VLC 转码，m3u8/HLS 直接通过 hls.js 播放
              const needsVlc = url.startsWith("rtmp://") || url.startsWith("rtmps://");
              const playUrl: string = needsVlc ? await invoke("vlc_play", { url }) : url;
              setVlcPlaying(needsVlc);
              if (item) {
                setPlayingList(orderedItems.length > 0 ? orderedItems : [item]);
                setPlayingIndex(Math.max(0, startIndex));
                setPlayingUrl(playUrl);
              }
              dispatchToast(
                <Toast><ToastTitle>已启动内置{needsVlc ? ' VLC 转码' : ' HLS'}播放</ToastTitle></Toast>,
                { intent: "info" }
              );
          } else {
            // use external player (PotPlayer, etc.)
            await invoke("open_in_player", { url });
            dispatchToast(
              <Toast><ToastTitle>正在启动播放器</ToastTitle></Toast>,
              { intent: "info" }
            );
          }
        }
      }
    } catch (e) {
      const errMsg = e instanceof Error ? e.message : String(e);
      console.error("handlePlayOrCopy 错误:", e);
      dispatchToast(
        <Toast><ToastTitle>播放失败: {errMsg}</ToastTitle></Toast>,
        { intent: "error" }
      );
    }
  };

  const handleCancelDownload = async (liveId: string) => {
    try {
      await invoke("cancel_download", { liveId });
      // 立即从 UI 中移除进度条
      setDownloadProgresses(prev => {
        const next = { ...prev };
        delete next[liveId];
        return next;
      });
      dispatchToast(
        <Toast><ToastTitle>已发送取消下载请求</ToastTitle></Toast>,
        { intent: "info" }
      );
    } catch (e) {
      console.error(e);
      dispatchToast(
        <Toast><ToastTitle>取消失败</ToastTitle></Toast>,
        { intent: "error" }
      );
    }
  };

  const resolveImgInfo = (path: string) => {
    if (path.startsWith("http")) return path;
    return "https://source3.48.cn" + path;
  };

  const renderGrid = (items: LiveItem[], isRecordTab: boolean) => {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 20, width: "100%", flexShrink: 0 }}>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: 20 }}>
          {items.map((item) => {
            const badge = getLiveTypeBadge(item.liveType);
            const isItemLiveTab = !isRecordTab;
            return (
              <div key={item.liveId} style={{ width: "100%", display: "flex", flexDirection: "column", gap: 0 }}>
                <Card style={{ width: "100%" }}>
                  <CardPreview>
                    <div style={{ position: "relative", width: "100%", lineHeight: 0 }}>
                      <img src={resolveImgInfo(item.coverPath)} alt="cover" style={{ objectFit: 'cover', width: '100%', aspectRatio: '1 / 1', display: 'block' }} />
                      
                      {item.inMicrophoneConnection && (
                        <div style={{
                          position: "absolute",
                          top: "12px",
                          right: "12px",
                          backgroundColor: "gray",
                          color: "white",
                          padding: "2px 6px",
                          fontSize: "12px",
                          fontWeight: "bold",
                          borderRadius: "4px",
                          lineHeight: "normal"
                        }}>
                          连麦
                        </div>
                      )}
                      
                      <div style={{
                          position: "absolute",
                          top: "12px",
                          left: "12px",
                          backgroundColor: isItemLiveTab ? "var(--colorPaletteRedBackground3)" : "#107c10",
                          color: "white",
                          padding: "4px 8px",
                          fontSize: "12px",
                          fontWeight: "bold",
                          borderRadius: "4px",
                          lineHeight: "normal",
                          boxShadow: "0 2px 4px rgba(0,0,0,0.2)"
                        }}>
                          {isItemLiveTab ? "直播" : "录播"}
                        </div>

                      <div style={{
                        position: "absolute",
                        bottom: 0,
                        left: 0,
                        width: "100%",
                        boxSizing: "border-box",
                        background: "linear-gradient(to top, rgba(0,0,0,0.8) 0%, transparent 100%)",
                        padding: "30px 12px 12px 12px",
                        display: "flex",
                        alignItems: "center",
                        gap: "8px",
                        lineHeight: "normal"
                      }}>
                        <Text weight="semibold" style={{ color: "white", flex: 1, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                          {item.title || "暂无标题"}
                        </Text>
                        <div style={{
                          backgroundColor: badge.color,
                          color: "white",
                          padding: "2px 6px",
                          fontSize: "12px",
                          fontWeight: "bold",
                          flexShrink: 0,
                          borderRadius: "4px"
                        }}>
                          {badge.text}
                        </div>
                      </div>
                    </div>
                  </CardPreview>

                  <CardHeader
                    header={<Text size={300} weight="semibold">{item.userInfo.nickname}</Text>}
                    description={item.ctime ? <Text size={200} style={{ color: "var(--colorNeutralForeground3)" }}>{dayjs(Number(item.ctime)).format("YYYY-MM-DD HH:mm:ss")}</Text> : undefined}
                    image={<Avatar image={{ src: resolveImgInfo(item.userInfo.avatar) }} />}
                  />

                  <div
                    style={{
                      display: "flex",
                      marginTop: "12px",
                      justifyContent: isItemLiveTab ? "flex-end" : "center",
                      gap: "8px"
                    }}
                  >
                    <Button 
                      icon={<CopyRegular />} 
                      onClick={() => handlePlayOrCopy(item.liveId, "copy", item)}
                    >地址</Button>
                    {isRecordTab && (
                      <Button 
                        icon={<ArrowDownloadRegular />} 
                        onClick={() => handlePlayOrCopy(item.liveId, "download", item)}
                      >下载</Button>
                    )}
                    <Button 
                      appearance="primary" 
                      icon={<PlayRegular />} 
                      onClick={() => handlePlayOrCopy(item.liveId, "play", item)}
                    >播放</Button>
                  </div>
                </Card>
                {downloadProgresses[item.liveId] !== undefined && (
                  <div style={{ width: "100%", marginTop: "4px" }}>
                    <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
                      <ProgressBar
                        value={downloadProgresses[item.liveId] <= 0 ? 0.02 : downloadProgresses[item.liveId] / 100}
                        style={{ flex: 1 }}
                      />
                      <Button
                        size="small"
                        icon={<DismissRegular />}
                        onClick={() => handleCancelDownload(item.liveId)}
                        title="取消下载"
                      />
                    </div>
                    <Text size={200} style={{ display: "block", textAlign: "center", marginTop: "4px", color: "var(--colorNeutralForeground3)" }}>
                      {downloadProgresses[item.liveId] > 0 ? `下载处理中... ${downloadProgresses[item.liveId].toFixed(2)}%` : "准备下载..."}
                    </Text>
                  </div>
                )}
              </div>
            );
          })}
        </div>
        {isRecordTab && recordNext !== "0" && (
          <div style={{ display: "flex", justifyContent: "center", marginTop: 20 }}>
            <Button onClick={loadMoreRecords} disabled={loading}>
              {loading ? "加载中..." : "加载更多录播"}
            </Button>
          </div>
        )}
      </div>
    );
  };

  const playingItem = playingIndex >= 0 ? playingList[playingIndex] : null;
  const pullProgress = Math.min(1, pullDistance / 56);

  return (
      <div
        id="app-root"
        ref={rootRef}
        style={{ padding: "0 20px 20px 20px", minHeight: "100vh", boxSizing: "border-box" }}
        onTouchStart={handleListTouchStart}
        onTouchEnd={handleListTouchEnd}
        onTouchCancel={handleListTouchEnd}
      >
        <Toaster toasterId={toasterId} offset={{ vertical: 96 }} />
        {isMobile && (
          <div
            style={{
              position: "fixed",
              top: 6,
              left: "50%",
              transform: `translate(-50%, ${Math.max(0, pullDistance - 24)}px)`,
              opacity: pullDistance > 0 || pullRefreshing ? 1 : 0,
              transition: gestureRef.current.tracking ? "none" : "transform 0.3s ease, opacity 0.2s ease",
              zIndex: 1001,
              pointerEvents: "none",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <div
              style={{
                width: "48px",
                height: "48px",
                borderRadius: "50%",
                background: "rgb(64, 64, 64)",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
              }}
            >
              <ArrowClockwiseRegular
                style={{
                  fontSize: "28px",
                  color: "#fff",
                  transform: `rotate(${pullRefreshing ? 0 : pullProgress * 300}deg)`,
                  transition: pullRefreshing ? "none" : "transform 0.08s linear",
                  animation: pullRefreshing ? "pull-refresh-spin 0.9s linear infinite" : "none",
                }}
              />
            </div>
          </div>
        )}
        
        <div style={{ 
          display: "flex", 
          flexDirection: "column",
          gap: "16px",
          marginBottom: 20,
          position: "sticky",
          top: 0,
          zIndex: 10,
          padding: "32px 0 10px 0",
          backgroundColor: "rgb(32, 32, 32)"
        }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <Text size={600} weight="semibold" style={{ flexShrink: 0, marginLeft: 10 }}>口袋48直播工具</Text>
              <Button
                appearance="subtle"
                icon={<SettingsRegular />}
                onClick={() => setSettingsOpen(true)}
                title="播放设置"
                aria-label="播放设置"
                style={{ minWidth: "36px", padding: "4px" }}
              />
            </div>

            {/* Mobile filter toggle (hidden on desktop via CSS) */}
            <Button 
              id="filter-toggle-btn"
              icon={<FilterRegular />}
              appearance="outline"
              onClick={() => setFilterOpen(prev => !prev)}
              style={{ marginRight: 6 }}
            >
              筛选
            </Button>
          </div>

          <div id="filter-bar" className={filterOpen ? "open" : ""} style={{ 
            display: "flex", 
            gap: "12px", 
            flexWrap: "wrap",
            alignItems: "center"
          }}>
            <TabList 
              selectedValue={activeTab} 
              onTabSelect={(_, data) => setActiveTab(data.value as any)}
              style={{ flexShrink: 0 }}
            >
              <Tab value="live">直播</Tab>
              <Tab value="record">录播</Tab>
            </TabList>

            <div style={{ display: "flex", gap: "12px", flex: 1, justifyContent: "flex-end", alignItems: "center", flexWrap: "wrap" }}>
              
              <Select 
              value={filterGroup} 
              onChange={(_, data) => setFilterGroup(data.value)}
              style={{ minWidth: "120px" }}
            >
              <option value="全部">全部团体</option>
              <option value="SNH48">SNH48</option>
              <option value="GNZ48">GNZ48</option>
              <option value="BEJ48">BEJ48</option>
              <option value="CKG48">CKG48</option>
              <option value="CGT48">CGT48</option>
            </Select>

            <Select 
              value={filterType} 
              onChange={(_, data) => setFilterType(data.value)}
              style={{ minWidth: "120px" }}
            >
              <option value="全部">全部类型</option>
              <option value="视频">仅视频</option>
              <option value="电台">仅电台</option>
              <option value="连麦">仅连麦</option>
            </Select>

            <Input 
              contentBefore={<SearchRegular />}
              placeholder="搜索成员或标题..." 
              value={searchQuery}
              onChange={(_, data) => setSearchQuery(data.value)}
              style={{ maxWidth: "300px", width: "100%" }}
            />
            <Button icon={<ArrowClockwiseRegular />} onClick={() => fetchLiveList()} disabled={loading}>
              {loading ? "刷新中..." : "刷新列表"}
            </Button>
          </div>
          </div>
        </div>

        {/* Bottom Navigation - Material Design Tab (Mobile only) */}
        <nav id="bottom-nav" className="bottom-nav">
          <button 
            className={`bottom-nav-item ${activeTab === "live" ? "active" : ""}`}
            onClick={() => setActiveTab("live")}
          >
            <svg className="bottom-nav-icon" viewBox="0 0 24 24" fill="currentColor">
              <path d="M17 10.5V7c0-.55-.45-1-1-1H4c-.55 0-1 .45-1 1v10c0 .55.45 1 1 1h12c.55 0 1-.45 1-1v-3.5l4 4v-11l-4 4z"/>
            </svg>
            <span className="bottom-nav-label">直播</span>
          </button>
          <button 
            className={`bottom-nav-item ${activeTab === "record" ? "active" : ""}`}
            onClick={() => setActiveTab("record")}
          >
            <svg className="bottom-nav-icon" viewBox="0 0 24 24" fill="currentColor">
              <path d="M18 4l2 4h-3l-2-4h-2l2 4h-3l-2-4H8l2 4H7L5 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V4h-4z"/>
            </svg>
            <span className="bottom-nav-label">录播</span>
          </button>
        </nav>

      <div style={{ overflow: "hidden", width: "100%" }}>
        <div style={{
          display: "flex",
          alignItems: "flex-start",
          width: "200%",
          transform: `translateX(calc(${activeTab === "live" ? 0 : -50}% + ${swipeTranslation}px))`,
          transition: gestureRef.current.tracking ? "none" : "transform 0.3s cubic-bezier(0.25, 0.8, 0.25, 1)",
        }}>
          <div style={{ 
            width: "50%", 
            paddingRight: "10px", 
            boxSizing: "border-box",
            height: activeTab === "live" ? "auto" : 0
          }}>
            {loading && lives.length === 0 ? (
              <Spinner label="加载列表中..." />
            ) : filteredLiveItems.length === 0 ? (
              <div style={{ textAlign: "center", marginTop: "40px", color: "var(--colorNeutralForeground3)" }}>无匹配内容</div>
            ) : (
              renderGrid(filteredLiveItems, false)
            )}
          </div>
          <div style={{ 
            width: "50%", 
            paddingLeft: "10px", 
            boxSizing: "border-box",
            height: activeTab === "record" ? "auto" : 0
          }}>
            {loading && records.length === 0 ? (
              <Spinner label="加载列表中..." />
            ) : filteredRecordItems.length === 0 ? (
              <div style={{ textAlign: "center", marginTop: "40px", color: "var(--colorNeutralForeground3)" }}>无匹配内容</div>
            ) : (
              renderGrid(filteredRecordItems, true)
            )}
          </div>
        </div>
      </div>

      {/* In-app video player (used on mobile/Android WebView) */}
      {playingItem && (
        <VideoPlayer
          liveId={playingItem.liveId}
          title={playingItem.title || "无标题"}
          nickname={playingItem.userInfo.nickname}
          streamUrl={playingUrl}
          isLive={activeTab === "live"}
          onSwitch={switchWebPlayback}
          onClose={() => {
            setPlayingIndex(-1);
            setPlayingList([]);
            setPlayingUrl("");
          }}
        />
      )}

      <SettingsPanel
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        useBuiltInVlc={useBuiltInVlc}
        onUseBuiltInVlcChange={handleUseBuiltInVlcChange}
        vlcPlaying={vlcPlaying}
        onVlcStop={handleVlcStop}
      />

      <style>{`
        @keyframes pull-refresh-spin {
          from { transform: rotate(0deg); }
          to { transform: rotate(360deg); }
        }
      `}</style>
    </div>
  );
}

export default App;
