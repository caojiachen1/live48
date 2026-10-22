# live48

A lightweight desktop client for Pocket48 (SNH48 Group) live streams. Native Rust
UI built on [egui](https://github.com/emilk/egui); playback runs in-process through
libVLC, so RTMP / HLS / HTTP-TS streams open directly with no local relay.

## Build

```bash
pwsh scripts/fetch-vlc.ps1        # libVLC runtime into ./vlc (gitignored)
cargo run --release
```

The release binary is `target/release/live48-egui.exe`. A debug build is
several times slower to draw, so use `--release`.

Settings, downloads and the cached engine live in `%APPDATA%/live48-egui/`.

## Checking without opening the UI

```bash
live48-egui --selftest <file:///path/to/video.ts>   # libVLC frame path
live48-egui --apitest                              # API + decode + reattach + seek
live48-egui --recordtest <mrl> <out.ts>            # libVLC stream-out capture
live48-egui --dltest <liveId> <out.ts> [秒]         # 分片下载 + 断点续传
L48_PERF=1 live48-egui                             # per-frame cost on stderr
```

## What it does

* 直播 / 录播 / 公演 / 公演录播 四个列表，公演走 `getOpenLiveList` + `getOpenLiveOne`
* 卡片区分类型、连麦与"录屏"（`liveMode=1`），录播显示时长
* 内置 libVLC 播放 RTMP / HLS / HTTP-TS，清晰度可选（标清/高清/超清，VIP 标注）
* 录播弹幕：读取 `msgFilePath` 的 `.lrc`，列表随进度高亮，点击跳转，可另存
* 录播下载：每个 TS 分片单独落到 `<目标>.cache/`，跳过已有分片，失败可"重试"续传，最后按字节合并
* 直播录制：后台一路 libVLC stream-out 写 TS，断流后 60 秒重新解析地址续录到 `(1)` 文件
* 自动抓取：按名单轮询直播列表，命中即录，最多 5 路，写 `pocket48_live_log.txt`
* 按成员向服务器查询历史录播（`userId` + 游标预处理）

Media requests go out as `User-Agent: SNH48 ENGINE`, which some record CDNs insist on.

## Branches

`main` is the egui client. The previous Tauri + React client is kept on `legacy`.

