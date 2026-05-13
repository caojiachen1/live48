import { useEffect, useState } from "react";
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
  Tab
} from "@fluentui/react-components";
import { PlayRegular, CopyRegular, ArrowClockwiseRegular, SearchRegular, ArrowDownloadRegular } from "@fluentui/react-icons";
import dayjs from "dayjs";

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
  const [filterGroup, setFilterGroup] = useState("全部");
  const [filterType, setFilterType] = useState("全部");
  
  const toasterId = useId("toaster");
  const { dispatchToast } = useToastController(toasterId);

  const fetchLiveList = async () => {
    setLoading(true);
    try {
      if (activeTab === "live") {
        const res: any = await invoke("get_live_list");
        if (res.content && res.content.liveList) {
          setLives(res.content.liveList);
        }
      } else {
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
      }
    } catch (e) {
      console.error(e);
      dispatchToast(
        <Toast><ToastTitle>获取列表失败</ToastTitle></Toast>,
        { intent: "error" }
      );
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
    fetchLiveList();
  }, [activeTab]);

  useEffect(() => {
    const unlisten = listen("download-status", (event: any) => {
      const payload = event.payload;
      if (payload.status === "start") {
        dispatchToast(<Toast><ToastTitle>正在下载：{payload.message}</ToastTitle></Toast>, { intent: "info" });
      } else if (payload.status === "success") {
        dispatchToast(<Toast><ToastTitle>下载完成：{payload.message}</ToastTitle></Toast>, { intent: "success" });
      } else if (payload.status === "error") {
        dispatchToast(<Toast><ToastTitle>下载失败：{payload.message}</ToastTitle></Toast>, { intent: "error" });
      }
    });

    return () => {
      unlisten.then(f => f());
    };
  }, []);

  const handlePlayOrCopy = async (liveId: string, action: "play" | "copy" | "download") => {
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
          await invoke("download_video", { url, savePath: filePath });
        }
      } else if (action === "play") {
        await invoke("open_in_player", { url });
        dispatchToast(
          <Toast><ToastTitle>正在启动播放器</ToastTitle></Toast>,
          { intent: "info" }
        );
      }
    } catch (e) {
      console.error(e);
      dispatchToast(
        <Toast><ToastTitle>获取(流/录播)地址失败</ToastTitle></Toast>,
        { intent: "error" }
      );
    }
  };

  const resolveImgInfo = (path: string) => {
    if (path.startsWith("http")) return path;
    return "https://source3.48.cn" + path;
  };

  const currentList = activeTab === "live" ? lives : records;

  const filteredItems = currentList.filter(item => {
    if (activeTab === "record" && targetLoadTime > 0 && item.ctime) {
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

  return (
    <div style={{ padding: "0 20px 20px 20px" }}>
      <Toaster toasterId={toasterId} />
      
      <div style={{ 
        display: "flex", 
        flexDirection: "column",
        gap: "16px",
        marginBottom: 20,
        position: "sticky",
        top: 0,
        zIndex: 10,
        padding: "20px 0 10px 0",
        backgroundColor: "rgb(32, 32, 32)"
      }}>
        <Text size={600} weight="semibold" style={{ flexShrink: 0 }}>口袋48 列表工具</Text>

        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: "16px" }}>
          <TabList 
            selectedValue={activeTab} 
            onTabSelect={(_, data) => setActiveTab(data.value as any)}
            style={{ flexShrink: 0 }}
          >
            <Tab value="live">直播</Tab>
            <Tab value="record">录播</Tab>
          </TabList>

          <div style={{ display: "flex", gap: "12px", flex: 1, justifyContent: "flex-end", alignItems: "center" }}>
            
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
          <Button icon={<ArrowClockwiseRegular />} onClick={fetchLiveList} disabled={loading}>
            {loading ? "刷新中..." : "刷新列表"}
          </Button>
        </div>
        </div>
      </div>

      {loading && currentList.length === 0 ? (
         <Spinner label="加载列表中..." />
      ) : filteredItems.length === 0 ? (
         <div style={{ textAlign: "center", marginTop: "40px", color: "var(--colorNeutralForeground3)" }}>无匹配内容</div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 20 }}>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: 20 }}>
            {filteredItems.map((item) => {
              const badge = getLiveTypeBadge(item.liveType);
              return (
              <Card key={item.liveId} style={{ width: "100%" }}>
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
                        backgroundColor: activeTab === "live" ? "var(--colorPaletteRedBackground3)" : "#107c10",
                        color: "white",
                        padding: "4px 8px",
                        fontSize: "12px",
                        fontWeight: "bold",
                        borderRadius: "4px",
                        lineHeight: "normal",
                        boxShadow: "0 2px 4px rgba(0,0,0,0.2)"
                      }}>
                        {activeTab === "live" ? "直播" : "录播"}
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
                    justifyContent: activeTab === "live" ? "flex-end" : "center",
                    gap: "8px"
                  }}
                >
                  <Button 
                    icon={<CopyRegular />} 
                    onClick={() => handlePlayOrCopy(item.liveId, "copy")}
                  >地址</Button>
                  {activeTab === "record" && (
                    <Button 
                      icon={<ArrowDownloadRegular />} 
                      onClick={() => handlePlayOrCopy(item.liveId, "download")}
                    >下载</Button>
                  )}
                  <Button 
                    appearance="primary" 
                    icon={<PlayRegular />} 
                    onClick={() => handlePlayOrCopy(item.liveId, "play")}
                  >播放</Button>
                </div>
              </Card>
              );
            })}
          </div>
          {activeTab === "record" && recordNext !== "0" && (
            <div style={{ display: "flex", justifyContent: "center", marginTop: 20 }}>
              <Button onClick={loadMoreRecords} disabled={loading}>
                {loading ? "加载中..." : "加载更多录播"}
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default App;