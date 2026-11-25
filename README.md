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
live48-egui --closetest <liveId> [open|member] [s] # open, decode, then time the teardown
live48-egui --cachetest <m3u8> [s]                 # cache, proxy, buffer end, retained folder
live48-egui --recordtest <mrl> <out.ts>            # libVLC stream-out capture
live48-egui --dltest <liveId> <out.ts> [seconds]   # segment download + resume
L48_PERF=1 live48-egui                             # per-frame cost on stderr
```

## What it does

* Four lists — live, records, theatre shows, and theatre archives; shows go through
  `getOpenLiveList` + `getOpenLiveOne`, and the live tab can be narrowed by team
  (`teamId`) from chips built out of the loaded rows
* Cards are tagged by type, mic connection, and "screen recording" (`liveMode=1`);
  records show their duration, theatre shows their play count
* Built-in libVLC plays RTMP / HLS / HTTP-TS, with selectable quality (SD / HD / UHD,
  VIP entries flagged). The address is resolved the way the app does it: merged
  multi-CDN `mergeStreamUrl` first, then `playStreamPath`, then `m3u8path`
* Player chrome shows the room's 人气 (`onlineNum`), its announcement as an overlay,
  and for theatre shows 播放 / 赞 (`playNum` / `praiseNum`), plus an 应援榜 page in the
  drawer fed by `getOpenLiveMemberRank`
* Portrait rooms (`pictureOrientation != 1`) get a tall placeholder before the first
  frame instead of a wide one
* Records play through a local read-ahead cache: the playlist is fetched once, segments
  are downloaded into `%APPDATA%/live48-egui/playcache/` four minutes past the playhead
  by six parallel lanes, and a loopback HTTP server feeds libVLC from disk as the
  segments are still being written. The seek bar therefore has a second, lighter rail
  for what is already buffered (the way video sites show it). Watched segments are kept;
  settings shows the disk usage and clears folders that no open player is using. Live
  streams still go straight to the CDN
* One HTTP client backs every API call and one `getLiveOne` feeds both the address and
  the chrome, so opening a record no longer pays two handshakes and two identical
  requests; closing hands the libVLC session to a background reaper thread instead of
  stopping it on the UI thread
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
* Per-member lookup: `getLiveList` with `userId` plus the emerging-page cursor, merged
  with the member's `getHistoryRecord` theatre playbacks
* A dead live stream is re-requested automatically three times, ten seconds apart,
  and the error card has a 重试 button
* `get/all/ongoing/live/info` is read in one call, so a member's row can be marked
  as being on air and opened straight from the history view

Media requests go out as `User-Agent: SNH48 ENGINE`, which some record CDNs insist on.

Anonymous requests omit the app's `pa` signature header (it needs the bundled
`libencryptlib.so`); everything used here answers without it.

## Branches

`main` is the egui client. The previous Tauri + React client is kept on `legacy`.
