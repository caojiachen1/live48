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
live48-egui --dltest <liveId> <out.ts> [seconds]   # segment download + resume
L48_PERF=1 live48-egui                             # per-frame cost on stderr
```

## What it does

* Four lists — live, records, theatre shows, and theatre archives; shows go through
  `getOpenLiveList` + `getOpenLiveOne`
* Cards are tagged by type, mic connection, and "screen recording" (`liveMode=1`);
  records show their duration
* Built-in libVLC plays RTMP / HLS / HTTP-TS, with selectable quality (SD / HD / UHD,
  VIP entries flagged)
* Record danmaku: the `.lrc` at `msgFilePath` is read, highlighted against the
  playhead, seekable by click, and can be saved
* Record download: every TS segment lands in `<target>.cache/`, existing segments are
  skipped, a failed run resumes from the holes, and the pieces are byte-concatenated
  at the end
* Live recording: one libVLC stream-out writes TS in the background; after the stream
  drops the address is resolved again within 60 seconds and recording continues into a
  `(1)` file
* Auto capture: polls the live list against a watch list, records on a hit, up to 5
  streams at once, and logs to `pocket48_live_log.txt`
* Per-member record lookup against the server (`userId` plus cursor priming)

Media requests go out as `User-Agent: SNH48 ENGINE`, which some record CDNs insist on.

## Branches

`main` is the egui client. The previous Tauri + React client is kept on `legacy`.
