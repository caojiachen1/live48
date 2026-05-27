import { useCallback, useEffect, useState } from "react";
import { Switch, Text, Button } from "@fluentui/react-components";
import { DismissRegular, SettingsRegular } from "@fluentui/react-icons";
import { invoke } from "@tauri-apps/api/core";

interface SettingsPanelProps {
  open: boolean;
  onClose: () => void;
  useBuiltInVlc: boolean;
  onUseBuiltInVlcChange: (value: boolean) => void;
  vlcPlaying: boolean;
  onVlcStop: () => void;
}

function SettingsPanel({ open, onClose, useBuiltInVlc, onUseBuiltInVlcChange, vlcPlaying, onVlcStop }: SettingsPanelProps) {
  const [vlcAvailable, setVlcAvailable] = useState<boolean | null>(null);

  // Check VLC availability on open
  useEffect(() => {
    if (open) {
      invoke<boolean>("check_vlc_installed")
        .then(setVlcAvailable)
        .catch(() => setVlcAvailable(false));
    }
  }, [open]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === "Escape") onClose();
  }, [onClose]);

  if (!open) return null;

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
          {/* VLC Switch */}
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
            <Switch
              label="使用 VLC 内置播放器"
              checked={useBuiltInVlc}
              onChange={(_, data) => onUseBuiltInVlcChange(data.checked as boolean)}
            />
            <Text size={200} style={{ color: "var(--colorNeutralForeground3)", lineHeight: 1.5 }}>
              开启后，桌面端播放直播/录播时将使用 libVLC 引擎在程序
              内部窗口直接播放，支持 RTMP 协议流，无需外部播放器。
              VLC 引擎会在首次使用时自动下载。
            </Text>
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
                  backgroundColor:
                    vlcAvailable === null
                      ? "rgba(255,255,255,0.3)"
                      : vlcAvailable
                      ? "#4caf50"
                      : "#ff5252",
                  flexShrink: 0,
                }}
              />
              <Text size={200}>
                {vlcAvailable === null
                  ? "正在检测 libVLC 引擎..."
                  : vlcAvailable
                  ? "libVLC 引擎可用"
                  : "libVLC 引擎不可用，请安装 VLC"}
              </Text>
            </div>
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
