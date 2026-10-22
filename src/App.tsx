import { useEffect, useMemo, useRef, useState } from "react";
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
import { PlayRegular, CopyRegular, ArrowClockwiseRegular, SearchRegular, ArrowDownloadRegular, DismissRegular, FilterRegular, SettingsRegular, RecordRegular, StopRegular, NotepadRegular } from "@fluentui/react-icons";
import VideoPlayer from "./VideoPlayer";
import MiniPlayer from "./MiniPlayer";
import FilterPanel, { GROUPS, TEAM_GROUPS } from "./FilterPanel";
import SettingsPanel, { loadAutoGrabConfig, saveAutoGrabConfig } from "./SettingsPanel";
import type { PlayerMode, AutoGrabConfig } from "./SettingsPanel";
import dayjs from "dayjs";
import "./App.android.css";

interface NativeAndroidPlayerBridge {
  isAvailable?: () => boolean;
  playStream: (url: string, title: string, nickname: string, isLive: boolean) => void;
  playPlaylist?: (playlistJson: string, startIndex: number, isLive: boolean) => void;
}

type TabKey = "live" | "record" | "open";
type ItemKind = "pocket" | "open";

interface LiveItem {
  liveId: string;
  title: string;
  coverPath: string;
  liveType: number;
  liveMode?: number;
  roomId?: string;
  status?: number;
  inMicrophoneConnection?: boolean;
  ctime?: string;
  kind?: ItemKind;
  userInfo: {
    nickname: string;
    avatar: string;
    userId?: string;
  };
}

interface OpenLiveItem {
  liveId: string;
  title: string;
  subTitle?: string;
  status: number;
  stime?: string;
  kind: ItemKind;
}

interface LiveRecordStatus {
  liveId: string;
  status: "recording" | "retrying" | "stopped" | "error" | string;
  filePath: string;
  elapsed: number;
  message: string;
}

interface OpenLiveStream {
  streamName: string;
  streamType: number;
  streamPath: string;
  vipShow: boolean;
}

const TAB_ORDER: TabKey[] = ["live", "record", "open"];

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

// liveMode 1 是录屏，优先级高于类型标签
const getItemBadge = (item: LiveItem) =>
  item.liveMode === 1 ? { text: "录屏", color: "blue" } : getLiveTypeBadge(item.liveType);

const FILE_TIME_FORMAT = "YYYY-MM-DD~HH.mm.ss";
const ILLEGAL_FILE_CHARS = /[\\/:*?"<>|]/g;

// [类型]_昵称_标题_开播时间__创建时间.ext
function buildFileName(
  typeTitle: string,
  nickname: string,
  title: string,
  openTime: string | number | undefined,
  ext: string
): string {
  const safeNickname = (nickname || "未知").replace(ILLEGAL_FILE_CHARS, "");
  const safeTitle = (title || "无标题").replace(ILLEGAL_FILE_CHARS, "");
  const start = openTime ? dayjs(Number(openTime)).format(FILE_TIME_FORMAT) : dayjs().format(FILE_TIME_FORMAT);
  const created = dayjs().format(FILE_TIME_FORMAT);
  return `[${typeTitle}]_${safeNickname}_${safeTitle}_${start}__${created}.${ext}`;
}

function joinPath(dir: string, name: string): string {
  return `${dir.replace(/[\\/]+$/, "")}/${name}`;
}

function formatElapsed(seconds: number): string {
  const total = Math.floor(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (v: number) => String(v).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

function App() {
  // Mini-player mode: render standalone MiniPlayer instead of full app
  const isMiniPlayer = new URLSearchParams(window.location.search).get("mini") === "true";
  if (isMiniPlayer) {
    return <MiniPlayer />;
  }

  const [activeTab, setActiveTab] = useState<TabKey>("live");
  const [lives, setLives] = useState<LiveItem[]>([]);
  const [records, setRecords] = useState<LiveItem[]>([]);
  const [recordNext, setRecordNext] = useState<string>("0");
  const [openLives, setOpenLives] = useState<OpenLiveItem[]>([]);
  const [openRecords, setOpenRecords] = useState<OpenLiveItem[]>([]);
  const [openRecordNext, setOpenRecordNext] = useState<string>("0");
  const [openGroupId, setOpenGroupId] = useState<number>(0);
  const [targetLoadTime, setTargetLoadTime] = useState<number>(0);
  const [loading, setLoading] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [downloadProgresses, setDownloadProgresses] = useState<Record<string, number>>({});
  const [downloadTasks, setDownloadTasks] = useState<Record<string, { url: string; savePath: string }>>({});
  const [downloadErrors, setDownloadErrors] = useState<Record<string, string>>({});
  const [filterGroups, setFilterGroups] = useState<string[]>([]);
  const [filterTypes, setFilterTypes] = useState<string[]>([]);
  const [filterMembers, setFilterMembers] = useState<string[]>([]);
  const [onlyMember, setOnlyMember] = useState<{ nickname: string; userId: string } | null>(null);
  const [filterDateStart, setFilterDateStart] = useState("");
  const [filterDateEnd, setFilterDateEnd] = useState("");
  const [filterPanelOpen, setFilterPanelOpen] = useState(false);
  const [pullDistance, setPullDistance] = useState(0);
  const [pullRefreshing, setPullRefreshing] = useState(false);
  const [swipeTranslation, setSwipeTranslation] = useState(0);
  const [recordings, setRecordings] = useState<Record<string, LiveRecordStatus>>({});
  
  // Settings state
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [playerMode, setPlayerMode] = useState<PlayerMode>(() => {
    const saved = localStorage.getItem("playerMode");
    if (saved === "vlc" || saved === "builtin" || saved === "system") return saved;
    // Migrate from old boolean setting
    if (localStorage.getItem("useBuiltInVlc") === "true") return "builtin";
    return "system";
  });
  const [vlcPlaying, setVlcPlaying] = useState(false);
  const [autoGrab, setAutoGrab] = useState<AutoGrabConfig>(() => loadAutoGrabConfig());
  const autoGrabRef = useRef<AutoGrabConfig>(autoGrab);
  autoGrabRef.current = autoGrab;
  const autoGrabRanRef = useRef(false);

  const handlePlayerModeChange = (mode: PlayerMode) => {
    setPlayerMode(mode);
    localStorage.setItem("playerMode", mode);
  };

  const handleAutoGrabChange = (config: AutoGrabConfig) => {
    setAutoGrab(config);
    saveAutoGrabConfig(config);
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
  const [playingIsLive, setPlayingIsLive] = useState<boolean>(true);
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
        setLives((res.content.liveList as LiveItem[]).map((item) => ({ ...item, kind: "pocket" as ItemKind })));
      }
    } catch (e) {
      console.error(e);
      dispatchToast(<Toast><ToastTitle>获取直播列表失败</ToastTitle></Toast>, { intent: "error" });
    }
  };

  // 只看某位成员时走服务端的 userId 查询，分页 next 沿用返回值
  const fetchRecordPage = async (
    next: string,
    member: { nickname: string; userId: string } | null = onlyMember
  ): Promise<any> => {
    if (member) {
      return await invoke("get_user_record_list", { userId: member.userId, next, record: true });
    }
    return await invoke("get_record_list", { next });
  };

  const fetchRecordInfo = async (
    member: { nickname: string; userId: string } | null = onlyMember
  ) => {
    try {
      const initialTargetTime = dayjs().subtract(1, 'day').startOf('day').valueOf();
      setTargetLoadTime(initialTargetTime);

      let currentNext = "0";
      let accumRecords: LiveItem[] = [];
      let done = false;

      while (!done) {
        const res: any = await fetchRecordPage(currentNext, member);
        if (res.content && res.content.liveList) {
          const newRecords: LiveItem[] = (res.content.liveList as LiveItem[])
            .map((item) => ({ ...item, kind: "pocket" as ItemKind }));
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

  const fetchOpenInfo = async (groupId: number = openGroupId) => {
    try {
      const [liveRes, recordRes]: any[] = await Promise.all([
        invoke("get_open_live_list", { next: "0", record: false, groupId }),
        invoke("get_open_live_list", { next: "0", record: true, groupId })
      ]);
      setOpenLives(((liveRes?.content?.liveList ?? []) as OpenLiveItem[]).map((item) => ({ ...item, kind: "open" as ItemKind })));
      setOpenRecords(((recordRes?.content?.liveList ?? []) as OpenLiveItem[]).map((item) => ({ ...item, kind: "open" as ItemKind })));
      setOpenRecordNext(recordRes?.content?.next || "0");
    } catch (e) {
      console.error(e);
      dispatchToast(<Toast><ToastTitle>获取公演列表失败</ToastTitle></Toast>, { intent: "error" });
    }
  };

  const loadMoreOpenRecords = async () => {
    if (openRecordNext === "0" || loading) return;
    setLoading(true);
    try {
      const res: any = await invoke("get_open_live_list", { next: openRecordNext, record: true, groupId: openGroupId });
      const list = ((res?.content?.liveList ?? []) as OpenLiveItem[]).map((item) => ({ ...item, kind: "open" as ItemKind }));
      setOpenRecords((prev) => [...prev, ...list]);
      setOpenRecordNext(res?.content?.next || "0");
    } catch (e) {
      console.error(e);
      dispatchToast(<Toast><ToastTitle>加载更多公演录播失败</ToastTitle></Toast>, { intent: "error" });
    } finally {
      setLoading(false);
    }
  };

  const fetchLiveList = async (tab?: TabKey) => {
    const target = tab || activeTab;
    setLoading(true);
    try {
      if (target === "live") {
        await fetchLiveInfo();
      } else if (target === "record") {
        await fetchRecordInfo();
      } else {
        await fetchOpenInfo();
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
        const res: any = await fetchRecordPage(currentNext);
        if (res.content && res.content.liveList) {
          const newRecords: LiveItem[] = (res.content.liveList as LiveItem[])
            .map((item) => ({ ...item, kind: "pocket" as ItemKind }));
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
      await Promise.all([fetchLiveInfo(), fetchRecordInfo(), fetchOpenInfo()]);
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
        setDownloadErrors(prev => {
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
        setDownloadErrors(prev => ({ ...prev, [payload.live_id]: payload.message }));
        dispatchToast(<Toast><ToastTitle>下载失败：{payload.message}</ToastTitle></Toast>, { intent: "error" });
      } else if (payload.status === "cancelled") {
        setDownloadProgresses(prev => {
          const next = { ...prev };
          delete next[payload.live_id];
          return next;
        });
        setDownloadErrors(prev => ({ ...prev, [payload.live_id]: payload.message }));
        dispatchToast(<Toast><ToastTitle>下载已取消：{payload.message}</ToastTitle></Toast>, { intent: "warning" });
      }
    });

    return () => {
      unlisten.then(f => f());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen("live-record-status", (event: any) => {
      const payload = event.payload as LiveRecordStatus;
      setRecordings((prev) => {
        const next = { ...prev };
        if (payload.status === "recording" || payload.status === "retrying") {
          next[payload.liveId] = payload;
        } else {
          delete next[payload.liveId];
        }
        return next;
      });
      if (payload.status === "error") {
        dispatchToast(<Toast><ToastTitle>录制失败：{payload.message}</ToastTitle></Toast>, { intent: "error" });
      } else if (payload.status === "stopped") {
        dispatchToast(<Toast><ToastTitle>录制已结束：{payload.filePath}</ToastTitle></Toast>, { intent: "warning" });
      }
    });

    return () => {
      unlisten.then(f => f());
    };
  }, []);

  // 已录时长靠轮询刷新，事件只在状态变化时来
  useEffect(() => {
    const syncRecordings = async () => {
      try {
        const list = await invoke<LiveRecordStatus[]>("get_live_record_list");
        const next: Record<string, LiveRecordStatus> = {};
        for (const item of list) next[item.liveId] = item;
        setRecordings(next);
      } catch (e) {
        console.error(e);
      }
    };
    syncRecordings();
    const timer = setInterval(syncRecordings, 2000);
    return () => clearInterval(timer);
  }, []);

  // 自动抓取（挂机录制）
  useEffect(() => {
    if (!autoGrab.enabled || !autoGrab.dir || !autoGrab.users.trim()) return;

    const run = async () => {
      const config = autoGrabRef.current;
      const names = config.users.split(",").map((s) => s.trim()).filter((s) => s.length > 0);
      if (names.length === 0) return;

      let active: LiveRecordStatus[] = [];
      try {
        active = await invoke<LiveRecordStatus[]>("get_live_record_list");
      } catch {
        return;
      }
      const recordingIds = new Set(active.map((r) => r.liveId));
      let running = active.length;

      const regexp = new RegExp(`(${names.map((n) => n.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "i");

      let res: any;
      try {
        res = await invoke("get_live_list");
      } catch (e) {
        console.error(e);
        return;
      }
      const list: LiveItem[] = (res?.content?.liveList ?? []) as LiveItem[];

      for (const item of list) {
        if (running >= 5) break;
        const matched = regexp.test(item.userInfo.nickname) || names.includes(item.userInfo.userId ?? "");
        if (!matched || recordingIds.has(item.liveId)) continue;

        try {
          const url: string = await invoke("get_live_stream_url", { liveId: item.liveId });
          const fileName = buildFileName("口袋48直播", item.userInfo.nickname, item.title, item.ctime, "ts");
          const savePath = joinPath(config.dir, fileName);
          const liveTime = item.ctime ? dayjs(Number(item.ctime)).format("YYYY-MM-DD HH:mm:ss") : "";
          const logContent = `直播标题：${item.title}
直播ID：${item.liveId}
直播人：${item.userInfo.nickname}
直播时间：${liveTime}
输出文件：${fileName}
创建时间：${dayjs().format("YYYY-MM-DD HH:mm:ss")}`;

          await invoke("append_record_log", { dir: config.dir, content: logContent });
          await invoke("start_live_record", { liveId: item.liveId, url, savePath });
          recordingIds.add(item.liveId);
          running += 1;
          dispatchToast(<Toast><ToastTitle>自动抓取已开录：{item.title}</ToastTitle></Toast>, { intent: "info" });
        } catch (e) {
          console.error(e);
          dispatchToast(<Toast><ToastTitle>自动抓取失败：{String(e)}</ToastTitle></Toast>, { intent: "error" });
        }
      }
    };

    if (autoGrab.enabled && !autoGrabRanRef.current) {
      autoGrabRanRef.current = true;
      run();
    } else if (!autoGrab.enabled) {
      autoGrabRanRef.current = false;
    }

    const interval = Math.max(1, autoGrab.intervalMinutes) * 60_000;
    const timer = setInterval(run, interval);
    return () => clearInterval(timer);
  }, [autoGrab.enabled, autoGrab.intervalMinutes, autoGrab.dir, autoGrab.users]);

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

      // Group filter: empty = all, otherwise match prefix
      const matchGroup = filterGroups.length === 0 ||
        filterGroups.some(g => item.userInfo.nickname.toUpperCase().startsWith(g));

      // Type filter: empty = all, otherwise match badge text
      let matchType = true;
      if (filterTypes.length > 0) {
        const typeText = getLiveTypeBadge(item.liveType).text;
        const byType = filterTypes.includes("连麦")
          ? (!!item.inMicrophoneConnection || filterTypes.includes(typeText))
          : filterTypes.includes(typeText);
        matchType = byType || (filterTypes.includes("录屏") && item.liveMode === 1);
      }

      // Member filter: empty = all, otherwise exact match
      const matchMember = filterMembers.length === 0 || filterMembers.includes(item.userInfo.nickname);

      // Date range filter (record only)
      let matchDate = true;
      if (isRecordTab && item.ctime) {
        const ts = Number(item.ctime);
        if (filterDateStart) {
          const start = new Date(filterDateStart).setHours(0, 0, 0, 0);
          if (ts < start) matchDate = false;
        }
        if (filterDateEnd) {
          const end = new Date(filterDateEnd).setHours(23, 59, 59, 999);
          if (ts > end) matchDate = false;
        }
      }

      return matchSearch && matchGroup && matchType && matchMember && matchDate;
    });
  };

  const toLiveItem = (item: OpenLiveItem): LiveItem => ({
    liveId: item.liveId,
    title: item.title,
    coverPath: "",
    liveType: 1,
    liveMode: 0,
    status: item.status,
    ctime: item.stime,
    kind: "open",
    userInfo: { nickname: item.subTitle || "公演", avatar: "" }
  });

  const filterOpenList = (list: OpenLiveItem[]) => {
    const q = searchQuery.toLowerCase();
    return list.filter((item) =>
      !q ||
      item.title.toLowerCase().includes(q) ||
      (item.subTitle ?? "").toLowerCase().includes(q)
    );
  };

  const filteredLiveItems = filterList(lives, false);
  const filteredRecordItems = filterList(records, true);
  const filteredOpenLiveItems = filterOpenList(openLives).map(toLiveItem);
  const filteredOpenRecordItems = filterOpenList(openRecords).map(toLiveItem);
  const filteredItems = activeTab === "live"
    ? filteredLiveItems
    : activeTab === "record"
      ? filteredRecordItems
      : [...filteredOpenLiveItems, ...filteredOpenRecordItems];

  // Derive unique members for the member filter (userId is needed for the server-side query)
  const allMembers = useMemo(() => {
    const source = activeTab === "live" ? lives : records;
    const map = new Map<string, string | undefined>();
    for (const item of source) {
      if (!map.has(item.userInfo.nickname)) map.set(item.userInfo.nickname, item.userInfo.userId);
    }
    return Array.from(map.entries())
      .map(([nickname, userId]) => ({ nickname, userId }))
      .sort((a, b) => a.nickname.localeCompare(b.nickname, "zh-Hans-CN"));
  }, [activeTab, lives, records]);

  // Whether any non-default filter is active
  const hasActiveFilters = filterGroups.length > 0 || filterTypes.length > 0 ||
    filterMembers.length > 0 || !!filterDateStart || !!filterDateEnd || !!onlyMember;

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
      const tabIndex = TAB_ORDER.indexOf(activeTab);
      if (tabIndex <= 0 && dx > 0) return; // 第一个 tab 不能再往右滑
      if (tabIndex >= TAB_ORDER.length - 1 && dx < 0) return; // 最后一个 tab 不能再往左滑
      
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
      const tabIndex = TAB_ORDER.indexOf(activeTab);
      if (swipeTranslation < -threshold && tabIndex < TAB_ORDER.length - 1) {
        setActiveTab(TAB_ORDER[tabIndex + 1]);
      } else if (swipeTranslation > threshold && tabIndex > 0) {
        setActiveTab(TAB_ORDER[tabIndex - 1]);
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

  // 成员直播/录播和公演走的是两套解析流地址的接口，用 kind 区分，不靠 liveId 猜
  const resolveStreamUrl = async (item: LiveItem): Promise<string> => {
    if (item.kind === "open") {
      const streams = await invoke<OpenLiveStream[]>("get_open_live_streams", { liveId: item.liveId });
      const best = [...streams].reverse().find((s) => !!s.streamPath);
      return best?.streamPath ?? "";
    }
    return await invoke<string>("get_live_stream_url", { liveId: item.liveId });
  };

  const switchWebPlayback = async (direction: "prev" | "next") => {
    if (playingIndex < 0 || playingList.length === 0) return;
    const nextIndex = direction === "next" ? playingIndex + 1 : playingIndex - 1;
    if (nextIndex < 0 || nextIndex >= playingList.length) {
      return direction === "next" ? "已经是最后一个了" : "已经是第一个了";
    }

    const nextItem = playingList[nextIndex];
    try {
      const nextUrl: string = await resolveStreamUrl(nextItem);
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

  const handleSelectPlaylistItem = async (index: number) => {
    if (index < 0 || index >= playingList.length || index === playingIndex) return;
    const item = playingList[index];
    try {
      const url: string = await resolveStreamUrl(item);
      setPlayingIndex(index);
      setPlayingUrl(url);
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

  const typeTitleOf = (kind: ItemKind | undefined, isLiveItem: boolean) =>
    kind === "open" ? (isLiveItem ? "48公演直播" : "48公演录播") : (isLiveItem ? "口袋48直播" : "口袋48录播");

  const handlePlayOrCopy = async (
    item: LiveItem,
    action: "play" | "copy" | "download",
    isLiveItem: boolean
  ) => {
    const liveId = item.liveId;
    try {
      const url: string = await resolveStreamUrl(item);
      
      if (action === "copy") {
        await writeText(url);
        dispatchToast(
          <Toast><ToastTitle>成功复制推流/录播地址</ToastTitle></Toast>,
          { intent: "success" }
        );
      } else if (action === "download") {
        if (!url) throw new Error("接口没有返回可下载的地址");
        const ext = /\.mp4(\?|$)/i.test(url) ? "mp4" : "ts";
        const filePath = await save({
          filters: [{ name: 'Video File', extensions: ['ts', 'mp4', 'flv'] }],
          defaultPath: buildFileName(
            typeTitleOf(item.kind, isLiveItem),
            item.userInfo.nickname,
            item.title,
            item.ctime,
            ext
          )
        });
        if (filePath) {
          // 记下地址和路径，失败或取消后"重试下载"可以原地续传
          setDownloadTasks(prev => ({ ...prev, [liveId]: { url, savePath: filePath } }));
          await invoke("download_video", { url, savePath: filePath, liveId });
        }
      } else if (action === "play") {
        const startIndex = filteredItems.findIndex(i => i.liveId === liveId);
        const orderedItems = startIndex >= 0 ? filteredItems : [item];

        // Android: all streams (live + record) use native player first
        if (isAndroid) {
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
                bridge.playPlaylist(JSON.stringify(playlistPayload), Math.max(0, startIndex), isLiveItem);
              } else {
                bridge.playStream(url, item.title || "无标题", item.userInfo.nickname, isLiveItem);
              }
              return;
            }
          }
        }

        setPlayingIsLive(isLiveItem);

        // Non-Android mobile fallback: in-app web player
        if (isMobile) {
          setPlayingList(orderedItems.length > 0 ? orderedItems : [item]);
          setPlayingIndex(Math.max(0, startIndex));
          setPlayingUrl(url);
          } else {
            // Desktop
            if (playerMode === "builtin") {
              // 仅 RTMP 需要 VLC 转码，m3u8/HLS 直接通过 hls.js 播放
              const needsVlc = url.startsWith("rtmp://") || url.startsWith("rtmps://");
              const playUrl: string = needsVlc ? await invoke("vlc_play", { url }) : url;
              setVlcPlaying(needsVlc);
              setPlayingList(orderedItems.length > 0 ? orderedItems : [item]);
              setPlayingIndex(Math.max(0, startIndex));
              setPlayingUrl(playUrl);
              dispatchToast(
                <Toast><ToastTitle>已启动内置{needsVlc ? ' VLC 转码' : ' HLS'}播放</ToastTitle></Toast>,
                { intent: "info" }
              );
          } else if (playerMode === "vlc") {
            await invoke("open_in_vlc", { url });
            dispatchToast(
              <Toast><ToastTitle>正在启动 VLC 播放器</ToastTitle></Toast>,
              { intent: "info" }
            );
          } else {
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

  const handleRetryDownload = async (liveId: string) => {
    const task = downloadTasks[liveId];
    if (!task) {
      dispatchToast(<Toast><ToastTitle>找不到原来的下载路径，请重新下载</ToastTitle></Toast>, { intent: "warning" });
      return;
    }
    try {
      await invoke("download_video", { url: task.url, savePath: task.savePath, liveId });
      dispatchToast(<Toast><ToastTitle>正在续传下载：{task.savePath}</ToastTitle></Toast>, { intent: "info" });
    } catch (e) {
      console.error(e);
      dispatchToast(<Toast><ToastTitle>重试下载失败</ToastTitle></Toast>, { intent: "error" });
    }
  };

  // 录播卡片上的"下载弹幕"，文件名沿用录播命名规则
  const handleDownloadDanmu = async (item: LiveItem) => {
    try {
      const filePath = await save({
        filters: [{ name: 'Danmu File', extensions: ['txt'] }],
        defaultPath: buildFileName("口袋48录播", item.userInfo.nickname, item.title, item.ctime, "txt")
      });
      if (!filePath) return;
      await invoke("download_danmu_file", { liveId: item.liveId, savePath: filePath });
      dispatchToast(<Toast><ToastTitle>弹幕文件已保存：{filePath}</ToastTitle></Toast>, { intent: "success" });
    } catch (e) {
      const errMsg = e instanceof Error ? e.message : String(e);
      console.error(e);
      dispatchToast(<Toast><ToastTitle>下载弹幕失败：{errMsg}</ToastTitle></Toast>, { intent: "error" });
    }
  };

  const handleToggleRecord = async (item: LiveItem) => {
    const existing = recordings[item.liveId];
    if (existing) {
      try {
        await invoke("stop_live_record", { liveId: item.liveId });
      } catch (e) {
        console.error(e);
        dispatchToast(<Toast><ToastTitle>停止录制失败</ToastTitle></Toast>, { intent: "error" });
      }
      return;
    }

    try {
      const url: string = await resolveStreamUrl(item);
      if (!url) throw new Error("接口没有返回可录制的地址");
      const filePath = await save({
        filters: [{ name: 'Video File', extensions: ['ts'] }],
        defaultPath: buildFileName(typeTitleOf(item.kind, true), item.userInfo.nickname, item.title, item.ctime, "ts")
      });
      if (!filePath) return;
      await invoke("start_live_record", { liveId: item.liveId, url, savePath: filePath });
      dispatchToast(<Toast><ToastTitle>已开始录制：{filePath}</ToastTitle></Toast>, { intent: "info" });
      const list = await invoke<LiveRecordStatus[]>("get_live_record_list");
      const next: Record<string, LiveRecordStatus> = {};
      for (const entry of list) next[entry.liveId] = entry;
      setRecordings(next);
    } catch (e) {
      const errMsg = e instanceof Error ? e.message : String(e);
      console.error(e);
      dispatchToast(<Toast><ToastTitle>录制失败：{errMsg}</ToastTitle></Toast>, { intent: "error" });
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
            const badge = getItemBadge(item);
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
                      flexWrap: "wrap",
                      justifyContent: isItemLiveTab ? "flex-end" : "center",
                      gap: "8px"
                    }}
                  >
                    <Button 
                      icon={<CopyRegular />} 
                      onClick={() => handlePlayOrCopy(item, "copy", isItemLiveTab)}
                    >地址</Button>
                    {isItemLiveTab ? (
                      !isAndroid && (recordings[item.liveId] ? (
                        <Button
                          appearance="primary"
                          icon={<StopRegular />}
                          onClick={() => handleToggleRecord(item)}
                        >停止录制</Button>
                      ) : (
                        <Button
                          icon={<RecordRegular />}
                          onClick={() => handleToggleRecord(item)}
                        >录制</Button>
                      ))
                    ) : (
                      <>
                        <Button 
                          icon={<ArrowDownloadRegular />} 
                          onClick={() => handlePlayOrCopy(item, "download", isItemLiveTab)}
                        >下载</Button>
                        <Button
                          icon={<NotepadRegular />}
                          onClick={() => handleDownloadDanmu(item)}
                        >弹幕</Button>
                      </>
                    )}
                    <Button 
                      appearance="primary" 
                      icon={<PlayRegular />} 
                      onClick={() => handlePlayOrCopy(item, "play", isItemLiveTab)}
                    >播放</Button>
                  </div>
                </Card>
                {recordings[item.liveId] && (
                  <div style={{ width: "100%", marginTop: "4px", padding: "4px 8px", backgroundColor: "rgba(191,54,54,0.15)", borderRadius: "4px" }}>
                    <Text size={200} style={{ display: "block", color: "#d13438" }}>
                      {recordings[item.liveId].status === "retrying" ? "录制中断，等待续录…" : "正在录制"} · {formatElapsed(recordings[item.liveId].elapsed)}
                    </Text>
                    <Text size={200} style={{ display: "block", color: "var(--colorNeutralForeground3)", wordBreak: "break-all" }}>
                      {recordings[item.liveId].filePath}
                    </Text>
                  </div>
                )}
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
                {downloadErrors[item.liveId] !== undefined && downloadProgresses[item.liveId] === undefined && (
                  <div style={{ width: "100%", marginTop: "4px", display: "flex", alignItems: "center", gap: "8px" }}>
                    <Text size={200} style={{ flex: 1, color: "#d13438", wordBreak: "break-all" }}>
                      {downloadErrors[item.liveId]}
                    </Text>
                    <Button
                      size="small"
                      icon={<ArrowClockwiseRegular />}
                      onClick={() => handleRetryDownload(item.liveId)}
                      title="重试下载"
                    >重试</Button>
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

  const renderOpenGrid = (items: LiveItem[], isLiveSection: boolean) => {
    if (items.length === 0) {
      return <div style={{ textAlign: "center", marginTop: "20px", color: "var(--colorNeutralForeground3)" }}>无匹配内容</div>;
    }
    return (
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: 20 }}>
        {items.map((item) => {
          const notStarted = item.status === 1;
          return (
            <Card key={item.liveId} style={{ width: "100%" }}>
              <CardPreview>
                <div style={{ position: "relative", width: "100%", lineHeight: 0, aspectRatio: "1 / 1", background: "linear-gradient(135deg, #2b2b2b 0%, #1a1a1a 100%)", display: "flex", alignItems: "center", justifyContent: "center", padding: "12px" }}>
                  <Text weight="bold" style={{ color: "rgba(255,255,255,0.85)", textAlign: "center", wordBreak: "break-all" }}>
                    {item.title || "公演"}
                  </Text>
                  <div style={{
                    position: "absolute",
                    top: "12px",
                    left: "12px",
                    backgroundColor: isLiveSection ? "var(--colorPaletteRedBackground3)" : "#107c10",
                    color: "white",
                    padding: "4px 8px",
                    fontSize: "12px",
                    fontWeight: "bold",
                    borderRadius: "4px",
                    lineHeight: "normal"
                  }}>
                    {isLiveSection ? "公演直播" : "公演录播"}
                  </div>
                  {notStarted && (
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
                      未开始
                    </div>
                  )}
                </div>
              </CardPreview>

              <CardHeader
                header={<Text size={300} weight="semibold">{item.userInfo.nickname}</Text>}
                description={item.ctime ? <Text size={200} style={{ color: "var(--colorNeutralForeground3)" }}>{dayjs(Number(item.ctime)).format("YYYY-MM-DD HH:mm:ss")}</Text> : undefined}
              />

              <div style={{ display: "flex", marginTop: "12px", justifyContent: "center", gap: "8px", flexWrap: "wrap" }}>
                <Button
                  appearance="primary"
                  icon={<PlayRegular />}
                  disabled={notStarted}
                  onClick={() => handlePlayOrCopy(item, "play", isLiveSection)}
                >播放</Button>
                {!isLiveSection && (
                  <Button
                    icon={<ArrowDownloadRegular />}
                    disabled={notStarted}
                    onClick={() => handlePlayOrCopy(item, "download", isLiveSection)}
                  >下载</Button>
                )}
              </div>
              {downloadProgresses[item.liveId] !== undefined && (
                <div style={{ display: "flex", alignItems: "center", gap: "8px", marginTop: "8px" }}>
                  <ProgressBar value={downloadProgresses[item.liveId] / 100} style={{ flex: 1 }} />
                  <Button size="small" icon={<DismissRegular />} onClick={() => handleCancelDownload(item.liveId)} title="取消下载" />
                </div>
              )}
              {downloadErrors[item.liveId] !== undefined && downloadProgresses[item.liveId] === undefined && (
                <div style={{ display: "flex", alignItems: "center", gap: "8px", marginTop: "8px" }}>
                  <Text size={200} style={{ flex: 1, color: "#d13438" }}>{downloadErrors[item.liveId]}</Text>
                  <Button size="small" icon={<ArrowClockwiseRegular />} onClick={() => handleRetryDownload(item.liveId)}>重试</Button>
                </div>
              )}
            </Card>
          );
        })}
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

            {/* Filter toggle button */}
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <Button
                id="filter-toggle-btn"
                icon={<FilterRegular />}
                appearance="outline"
                onClick={() => setFilterPanelOpen(true)}
                style={{ marginRight: 6, position: "relative" }}
              >
                筛选
                {hasActiveFilters && (
                  <span style={{
                    position: "absolute",
                    top: "4px",
                    right: "4px",
                    width: "8px",
                    height: "8px",
                    borderRadius: "50%",
                    backgroundColor: "#bb86fc",
                  }} />
                )}
              </Button>
            </div>
          </div>

          <div id="filter-bar" style={{
            display: "flex",
            gap: "12px",
            flexWrap: "wrap",
            alignItems: "center"
          }}>
            <TabList
              selectedValue={activeTab}
              onTabSelect={(_, data) => setActiveTab(data.value as TabKey)}
              style={{ flexShrink: 0 }}
            >
              <Tab value="live">直播</Tab>
              <Tab value="record">录播</Tab>
              <Tab value="open">公演</Tab>
            </TabList>

            <div style={{ display: "flex", gap: "12px", flex: 1, justifyContent: "flex-end", alignItems: "center", flexWrap: "wrap" }}>

            {activeTab === "open" ? (
              <Select
                value={String(openGroupId)}
                onChange={(_, data) => {
                  const id = Number(data.value);
                  setOpenGroupId(id);
                  fetchOpenInfo(id);
                }}
                style={{ minWidth: "120px" }}
              >
                <option value="0">全部团体</option>
                {GROUPS.map((g) => <option key={g.id} value={String(g.id)}>{g.name}</option>)}
              </Select>
            ) : (
            <Select
              value={filterGroups.length === 0 ? "全部" : filterGroups.length === 1 ? filterGroups[0] : "__multi__"}
              onChange={(_, data) => {
                if (data.value === "__multi__") return;
                setFilterGroups(data.value === "全部" ? [] : [data.value]);
              }}
              style={{ minWidth: "120px" }}
            >
              <option value="全部">全部团体</option>
              {TEAM_GROUPS.map((g) => <option key={g.id} value={g.name}>{g.name}</option>)}
              {filterGroups.length > 1 && <option value="__multi__">已选 {filterGroups.length} 个</option>}
            </Select>
            )}

            <Select
              value={filterTypes.length === 0 ? "全部" : filterTypes.length === 1 ? filterTypes[0] : "__multi__"}
              onChange={(_, data) => {
                if (data.value === "__multi__") return;
                setFilterTypes(data.value === "全部" ? [] : [data.value]);
              }}
              style={{ minWidth: "120px" }}
            >
              <option value="全部">全部类型</option>
              <option value="视频">仅视频</option>
              <option value="电台">仅电台</option>
              <option value="连麦">仅连麦</option>
              <option value="录屏">仅录屏</option>
              <option value="游戏">仅游戏</option>
              <option value="AI">仅AI</option>
              {filterTypes.length > 1 && <option value="__multi__">已选 {filterTypes.length} 个</option>}
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
          <button 
            className={`bottom-nav-item ${activeTab === "open" ? "active" : ""}`}
            onClick={() => setActiveTab("open")}
          >
            <svg className="bottom-nav-icon" viewBox="0 0 24 24" fill="currentColor">
              <path d="M12 2l3 5h5l-4 4 1.5 6L12 14l-5.5 3L8 7 4 3h5l3-1z"/>
            </svg>
            <span className="bottom-nav-label">公演</span>
          </button>
        </nav>

      <div style={{ overflow: "hidden", width: "100%" }}>
        <div style={{
          display: "flex",
          alignItems: "flex-start",
          width: "300%",
          transform: `translateX(calc(${-TAB_ORDER.indexOf(activeTab) * 33.3333}% + ${swipeTranslation}px))`,
          transition: gestureRef.current.tracking ? "none" : "transform 0.3s cubic-bezier(0.25, 0.8, 0.25, 1)",
        }}>
          <div style={{ 
            width: "33.3333%", 
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
            width: "33.3333%", 
            paddingLeft: "10px", 
            paddingRight: "10px", 
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
          <div style={{ 
            width: "33.3333%", 
            paddingLeft: "10px", 
            boxSizing: "border-box",
            height: activeTab === "open" ? "auto" : 0
          }}>
            <div style={{ display: "flex", flexDirection: "column", gap: 20, width: "100%" }}>
              {loading && openLives.length === 0 ? (
                <Spinner label="加载列表中..." />
              ) : (
                <>
                  <Text size={400} weight="semibold">公演直播</Text>
                  {renderOpenGrid(filteredOpenLiveItems, true)}
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                    <Text size={400} weight="semibold">公演录播</Text>
                    {openRecordNext !== "0" && (
                      <Button size="small" onClick={loadMoreOpenRecords} disabled={loading}>
                        {loading ? "加载中..." : "加载更多公演录播"}
                      </Button>
                    )}
                  </div>
                  {renderOpenGrid(filteredOpenRecordItems, false)}
                </>
              )}
            </div>
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
          kind={playingItem.kind ?? "pocket"}
          isLive={playingIsLive}
          isRecording={!!recordings[playingItem.liveId]}
          onToggleRecord={playingItem.kind === "open" ? undefined : () => handleToggleRecord(playingItem)}
          canSwitchPrev={playingIndex > 0}
          canSwitchNext={playingIndex < playingList.length - 1}
          onSwitch={switchWebPlayback}
          playlist={playingList}
          currentIndex={playingIndex}
          onSelect={handleSelectPlaylistItem}
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
        playerMode={playerMode}
        onPlayerModeChange={handlePlayerModeChange}
        vlcPlaying={vlcPlaying}
        onVlcStop={handleVlcStop}
        autoGrab={autoGrab}
        onAutoGrabChange={handleAutoGrabChange}
        recordingCount={Object.keys(recordings).length}
      />

      <FilterPanel
        open={filterPanelOpen}
        onClose={() => setFilterPanelOpen(false)}
        activeTab={activeTab}
        filterGroups={filterGroups}
        filterTypes={filterTypes}
        filterMembers={filterMembers}
        filterDateStart={filterDateStart}
        filterDateEnd={filterDateEnd}
        onGroupsChange={setFilterGroups}
        onTypesChange={setFilterTypes}
        onMembersChange={setFilterMembers}
        onDateStartChange={setFilterDateStart}
        onDateEndChange={setFilterDateEnd}
        allMembers={allMembers}
        onlyMember={onlyMember}
        onOnlyMemberChange={(value) => {
          setOnlyMember(value);
          fetchRecordInfo(value);
        }}
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
