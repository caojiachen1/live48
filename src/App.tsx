import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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
  Select
} from "@fluentui/react-components";
import { PlayRegular, CopyRegular, ArrowClockwiseRegular, SearchRegular } from "@fluentui/react-icons";

interface LiveItem {
  liveId: string;
  title: string;
  coverPath: string;
  liveType: number;
  inMicrophoneConnection?: boolean;
  userInfo: {
    nickname: string;
    avatar: string;
  };
}

const getLiveTypeBadge = (type: number) => {
  // 根据常见的口袋48 liveType: 1视频, 2电台, 3连麦可能
  let text = "未知";
  let color = "gray";
  if (type === 1) { text = "视频"; color = "purple"; }
  else if (type === 2) { text = "电台"; color = "#fecd00"; /* 偏黄色 */ }
  else if (type === 3) { text = "连麦"; color = "blue"; }
  else if (type === 5) { text = "游戏"; color = "green"; }
  else if (type === 6) { text = "AI"; color = "teal"; }

  return { text, color };
};

function App() {
  const [lives, setLives] = useState<LiveItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [filterGroup, setFilterGroup] = useState("全部");
  const [filterType, setFilterType] = useState("全部");
  
  const toasterId = useId("toaster");
  const { dispatchToast } = useToastController(toasterId);

  const fetchLiveList = async () => {
    setLoading(true);
    try {
      const res: any = await invoke("get_live_list");
      if (res.content && res.content.liveList) {
        setLives(res.content.liveList);
      }
    } catch (e) {
      console.error(e);
      dispatchToast(
        <Toast><ToastTitle>获取直播列表失败</ToastTitle></Toast>,
        { intent: "error" }
      );
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchLiveList();
  }, []);

  const handlePlayOrCopy = async (liveId: string, action: "play" | "copy") => {
    try {
      const url: string = await invoke("get_live_stream_url", { liveId });
      
      if (action === "copy") {
        await writeText(url);
        dispatchToast(
          <Toast><ToastTitle>成功复制推流地址</ToastTitle></Toast>,
          { intent: "success" }
        );
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
        <Toast><ToastTitle>获取流地址失败</ToastTitle></Toast>,
        { intent: "error" }
      );
    }
  };

  const resolveImgInfo = (path: string) => {
    if (path.startsWith("http")) return path;
    return "https://source3.48.cn" + path;
  };

  const filteredLives = lives.filter(item => {
    // 搜索过滤
    const matchSearch = item.userInfo.nickname.toLowerCase().includes(searchQuery.toLowerCase()) || 
      (item.title && item.title.toLowerCase().includes(searchQuery.toLowerCase()));

    // 团体过滤 (根据昵称前缀)
    const matchGroup = filterGroup === "全部" || item.userInfo.nickname.toUpperCase().startsWith(filterGroup);

    // 类型过滤
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
        justifyContent: "space-between", 
        alignItems: "center", 
        marginBottom: 20,
        position: "sticky",
        top: 0,
        zIndex: 10,
        padding: "20px 0",
        backgroundColor: "rgb(32, 32, 32)",
        gap: "16px"
      }}>
        <Text size={600} weight="semibold" style={{ flexShrink: 0 }}>口袋48 直播列表</Text>
        <div style={{ display: "flex", gap: "12px", flex: 1, justifyContent: "flex-end", alignItems: "center" }}>
          
          <Select 
            value={filterGroup} 
            onChange={(e, data) => setFilterGroup(data.value)}
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
            onChange={(e, data) => setFilterType(data.value)}
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
            onChange={(e, data) => setSearchQuery(data.value)}
            style={{ maxWidth: "300px", width: "100%" }}
          />
          <Button icon={<ArrowClockwiseRegular />} onClick={fetchLiveList} disabled={loading}>
            {loading ? "刷新中..." : "刷新列表"}
          </Button>
        </div>
      </div>

      {loading && lives.length === 0 ? (
         <Spinner label="加载房间列表..." />
      ) : filteredLives.length === 0 ? (
         <div style={{ textAlign: "center", marginTop: "40px", color: "var(--colorNeutralForeground3)" }}>无匹配的直播</div>
      ) : (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))", gap: 20 }}>
          {filteredLives.map((item) => {
            const badge = getLiveTypeBadge(item.liveType);
            return (
            <Card key={item.liveId} style={{ width: "100%" }}>
              <CardPreview>
                <div style={{ position: "relative", width: "100%", lineHeight: 0 }}>
                  <img src={resolveImgInfo(item.coverPath)} alt="cover" style={{ objectFit: 'cover', width: '100%', aspectRatio: '1 / 1', display: 'block' }} />
                  
                  {/* 连麦标识 - 右上角 */}
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

                  {/* 封面下方的阴影遮罩、标题和类型 */}
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
                image={
                  <Avatar image={{ src: resolveImgInfo(item.userInfo.avatar) }} />
                }
              />

              <div style={{ display: "flex", gap: "8px", marginTop: "12px", justifyContent: "flex-end" }}>
                <Button 
                  icon={<CopyRegular />} 
                  onClick={() => handlePlayOrCopy(item.liveId, "copy")}
                >复制链接</Button>
                <Button 
                  appearance="primary" 
                  icon={<PlayRegular />} 
                  onClick={() => handlePlayOrCopy(item.liveId, "play")}
                >用本地播放器打开</Button>
              </div>
            </Card>
            );
          })}
        </div>
      )}
    </div>
  );
}

export default App;

