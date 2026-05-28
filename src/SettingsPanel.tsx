import { useCallback, useEffect, useState } from "react";
import { Radio, RadioGroup, Text, Button, ProgressBar } from "@fluentui/react-components";
import { DismissRegular, SettingsRegular, ArrowDownloadRegular } from "@fluentui/react-icons";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type PlayerMode = "system" | "vlc" | "builtin";

interface SettingsPanelProps {
  open: boolean;
  onClose: () => void;
  playerMode: PlayerMode;
  onPlayerModeChange: (mode: PlayerMode) => void;
  vlcPlaying: boolean;
  onVlcStop: () => void;
}

interface VlcDownloadProgress {
  progress: number;
  message: string;
}

function SettingsPanel({ open, onClose, playerMode, onPlayerModeChange, vlcPlaying, onVlcStop }: SettingsPanelProps) {
  const [vlcAvailable, setVlcAvailable] = useState<boolean | null>(null);
  const [vlcIntegrated, setVlcIntegrated] = useState<boolean | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<VlcDownloadProgress | null>(null);

  // Re-check VLC status
  const checkVlcStatus = useCallback(async () => {
    try {
      const installed = await invoke<boolean>("check_vlc_installed");
      setVlcAvailable(installed);
      
      // Also check integrity to see if bundled VLC is complete
      try {
        await invoke<string>("check_vlc_integrity");
        setVlcIntegrated(true);
      } catch {
        setVlcIntegrated(false);
      }
    } catch {
      setVlcAvailable(false);
      setVlcIntegrated(false);
    }
  }, []);

  // Listen for VLC download progress events
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    const setup = async () => {
      const unlistenFn = await listen<VlcDownloadProgress>("vlc-download-progress", (event) => {
        const payload = event.payload;
        
        if (payload.progress === 0) {
          setDownloading(true);
          setDownloadProgress(payload);
        } else if (payload.progress === 100) {
          setDownloadProgress(payload);
          // Download completed successfully, re-check status
          setTimeout(() => {
            setDownloading(false);
            setDownloadProgress(null);
            checkVlcStatus();
          }, 1500);
        } else if (payload.progress < 0) {
          // Error
          setDownloadProgress(payload);
          setTimeout(() => {
            setDownloading(false);
            setDownloadProgress(null);
          }, 5000);
        } else {
          setDownloadProgress(payload);
        }
      });
      unlisten = unlistenFn;
    };
    setup();

    return () => {
      if (unlisten) unlisten();
    };
  }, [checkVlcStatus]);

  // Check VLC availability on open
  useEffect(() => {
    if (open) {
      checkVlcStatus();
    }
  }, [open, checkVlcStatus]);

  const handleDownloadVlc = async () => {
    setDownloading(true);
    setDownloadProgress({ progress: 0, message: "正在准备下载..." });
    try {
      await invoke("download_vlc");
    } catch (e) {
      console.error(e);
      setDownloadProgress({ progress: -1, message: `启动下载失败: ${e}` });
      setTimeout(() => {
        setDownloading(false);
        setDownloadProgress(null);
      }, 5000);
    }
  };

  const handlePlayerModeChange = (_: any, data: { value: string }) => {
    const mode = data.value as PlayerMode;
    if (mode === "builtin" && vlcIntegrated !== true) {
      return; // VLC not available, don't allow switching
    }
    onPlayerModeChange(mode);
  };

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  }, [onClose]);

  if (!open) return null;

  const builtinDisabled = vlcAvailable !== true || vlcIntegrated !== true;
  
  // Show appropriate status message
  let statusText = "";
  let statusColor = "rgba(255,255,255,0.3)";
  if (vlcAvailable === null) {
    statusText = "正在检测 libVLC 引擎...";
    statusColor = "rgba(255,255,255,0.3)";
  } else if (downloading) {
    statusText = downloadProgress?.message || "正在下载...";
    statusColor = "#4fc3f7";
  } else if (vlcAvailable && vlcIntegrated) {
    statusText = "libVLC 引擎可用 (已内置)";
    statusColor = "#4caf50";
  } else if (vlcAvailable && !vlcIntegrated) {
    statusText = "libVLC 引擎不可用 (系统安装但未内置)";
    statusColor = "#ff9800";
  } else {
    statusText = "libVLC 引擎不可用，请点击下载";
    statusColor = "#ff5252";
  }

  return (
    <>
      {/* Backdrop */}
      <div
        onClick={onClose}
        style={{
          position: "fixed",
          top: 0,
          left: 0,
          right: 0,
          bottom: 0,
          backgroundColor: "rgba(0,0,0,0.5)",
          zIndex: 5000,
          animation: "settings-fade-in 0.2s ease",
        }}
      />

      {/* Panel */}
      <div
        onKeyDown={handleKeyDown}
        role="dialog"
        aria-label="播放设置"
        style={{
          position: "fixed",
          top: 0,
          right: 0,
          bottom: 0,
          width: "min(380px, 85vw)",
          backgroundColor: "#1f1f1f",
          zIndex: 5001,
          display: "flex",
          flexDirection: "column",
          boxShadow: "-4px 0 24px rgba(0,0,0,0.4)",
          animation: "settings-slide-in 0.25s cubic-bezier(0.25, 0.8, 0.25, 1)",
          overflow: "hidden",
        }}
      >
        {/* Header */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "20px 20px 16px 20px",
            borderBottom: "1px solid rgba(255,255,255,0.08)",
            flexShrink: 0,
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: "10px" }}>
            <SettingsRegular style={{ fontSize: "22px", color: "#bb86fc" }} />
            <Text size={500} weight="semibold">播放设置</Text>
          </div>
          <Button
            appearance="subtle"
            icon={<DismissRegular />}
            onClick={onClose}
            aria-label="关闭设置"
          />
        </div>

        {/* Content */}
        <div
          style={{
            flex: 1,
            overflowY: "auto",
            padding: "20px",
            display: "flex",
            flexDirection: "column",
            gap: "24px",
          }}
        >
          {/* Player Mode Selection */}
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: "8px",
              padding: "16px",
              backgroundColor: "rgba(255,255,255,0.04)",
              borderRadius: "8px",
            }}
          >
            <Text size={300} weight="semibold" style={{ color: "rgba(255,255,255,0.7)" }}>
              播放器选择
            </Text>
            <RadioGroup
              value={playerMode}
              onChange={handlePlayerModeChange}
              style={{ display: "flex", flexDirection: "column", gap: "6px" }}
            >
              <Radio value="system" label="系统默认播放器" />
              <Text size={200} style={{ color: "var(--colorNeutralForeground3)", marginLeft: "24px", marginTop: "-4px" }}>
                使用 PotPlayer 或系统关联播放器打开
              </Text>
              <Radio value="vlc" label="VLC 播放器" />
              <Text size={200} style={{ color: "var(--colorNeutralForeground3)", marginLeft: "24px", marginTop: "-4px" }}>
                使用已安装的 VLC 播放器打开
              </Text>
              <Radio value="builtin" label="内置播放器 (libVLC)" disabled={builtinDisabled} />
              <Text size={200} style={{ color: "var(--colorNeutralForeground3)", marginLeft: "24px", marginTop: "-4px" }}>
                程序内直接播放，支持 RTMP 流，需下载 VLC 引擎
              </Text>
            </RadioGroup>
            {builtinDisabled && !downloading && playerMode !== "builtin" && (
              <Text size={200} style={{ color: "#ff9800", lineHeight: 1.5, marginTop: "4px" }}>
                内置播放器需要先下载 VLC 引擎
              </Text>
            )}
          </div>

          {/* VLC Status */}
          <div
            style={{
              padding: "12px 16px",
              borderRadius: "8px",
              backgroundColor: "rgba(255,255,255,0.04)",
              display: "flex",
              flexDirection: "column",
              gap: "12px",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: "10px" }}>
              <div
                style={{
                  width: "8px",
                  height: "8px",
                  borderRadius: "50%",
                  backgroundColor: statusColor,
                  flexShrink: 0,
                }}
              />
              <Text size={200} style={{ flex: 1 }}>
                {statusText}
              </Text>
            </div>

            {/* Download Progress */}
            {downloading && downloadProgress && (
              <div style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
                <ProgressBar
                  value={downloadProgress.progress > 0 ? Math.min(downloadProgress.progress / 100, 0.99) : undefined}
                  style={{ width: "100%" }}
                />
                <Text size={200} style={{ color: "var(--colorNeutralForeground3)", textAlign: "center" }}>
                  {downloadProgress.progress > 0
                    ? `${downloadProgress.progress.toFixed(1)}% - ${downloadProgress.message}`
                    : downloadProgress.message}
                </Text>
              </div>
            )}

            {/* Download Error */}
            {downloadProgress && downloadProgress.progress < 0 && (
              <Text size={200} style={{ color: "#ff5252", lineHeight: 1.4 }}>
                {downloadProgress.message}
              </Text>
            )}

            {/* Download Button - only show if VLC not available/integrated and not currently downloading */}
            {vlcIntegrated !== true && !downloading && (
              <Button
                appearance="primary"
                icon={<ArrowDownloadRegular />}
                onClick={handleDownloadVlc}
                style={{ width: "100%" }}
              >
                下载 VLC 引擎 (~101 MB)
              </Button>
            )}

            {/* VLC Stop Button */}
            {vlcPlaying && (
              <Button
                appearance="primary"
                icon={<DismissRegular />}
                onClick={onVlcStop}
                style={{ width: "100%" }}
              >
                停止 VLC 播放
              </Button>
            )}
          </div>
        </div>
      </div>

      {/* Animations */}
      <style>{`
        @keyframes settings-fade-in {
          from { opacity: 0; }
          to { opacity: 1; }
        }
        @keyframes settings-slide-in {
          from { transform: translateX(100%); }
          to { transform: translateX(0); }
        }
      `}</style>
    </>
  );
}

export default SettingsPanel;