# live48

A lightweight desktop client for Pocket48 (SNH48 Group) live streams. Native Rust
UI built on [egui](https://github.com/emilk/egui); playback runs in-process through
libVLC, so RTMP / HLS / HTTP-TS streams open directly with no local relay.

## Build

```bash
pwsh scripts/fetch-vlc.ps1        # libVLC runtime into ./vlc (gitignored)
cargo run --release --manifest-path src-egui/Cargo.toml
```

The release binary is `src-egui/target/release/live48-egui.exe`. A debug build is
several times slower to draw, so use `--release`.

Settings, downloads and the cached engine live in `%APPDATA%/live48-egui/`.

## Checking without opening the UI

```bash
live48-egui --selftest <file:///path/to/video.ts>   # libVLC frame path
live48-egui --apitest                              # API + decode + reattach + seek
L48_PERF=1 live48-egui                             # per-frame cost on stderr
```

## Branches

`main` is the egui client. The previous Tauri + React client is kept on `legacy`.
