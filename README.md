# live48

A lightweight Windows desktop client for Pocket48 (SNH48 Group) live streams.
Playback uses libVLC and opens RTMP / HLS / HTTP-TS streams directly.

## Build

```bash
pwsh scripts/fetch-vlc.ps1
cargo build --release
```

The release binary is `target/release/live48.exe`.

Settings, downloads and the cached engine stay in the app data folder. The built-in
player can also download its libVLC engine from Playback Settings.

## What it does

* Four tabs: Live, Recordings, Theatre and Theatre Archive. Search, filter by group,
  type, member or date, and narrow the Live tab by team.
* Cards are tagged by type, mic connection and screen recording; recordings show their
  duration, theatre shows their play count.
* Built-in libVLC plays RTMP / HLS / HTTP-TS, with selectable quality (SD / HD / UHD,
  VIP entries flagged).
* Player UI shows the room's popularity, its announcement in the black side rail beside
  the picture (closable with the close button on the rail, and restorable from the
  Show announcement button in the top bar), and for theatre shows play and like counts,
  plus a support-ranking page in the drawer.
* Portrait rooms get a tall placeholder before the first frame instead of a wide one.
* Recordings buffer ahead on disk. The seek bar has a second, lighter rail for the
  range that is already cached; watched segments are kept, and Playback Settings shows
  the disk usage and clears folders no open player is using. Live streams are not
  buffered on disk.
* Danmaku for recordings: lines from the accompanying LRC file are highlighted against
  the playhead, seekable by click, and can be saved.
* Downloading recordings saves a TS file, skips parts that are already present, and
  resumes a failed run from the missing pieces.
* Live recording: writes TS in the background; if the stream drops, the app reconnects
  and continues recording.
* Auto capture: polls the live list against a watch list, records on a hit, up to five
  streams at once, and logs to `pocket48_live_log.txt`.
* Per-member lookup: find a member, jump to their current live room, or show only their
  past recordings.
* A dead live stream is retried automatically, and the error card has a Retry button.
* Multi-view puts up to twelve live rooms in one window. The member drawer lists every
  room that is on air (independent of the list filters) and uses the member's live
  avatar in each row. The layout menu offers Auto, Two columns, Three columns and
  Focus; dragging a tile swaps positions, and dragging the gutters changes row and
  column sizes. Auto keeps the live aspect, puts up to four rooms in one row and uses
  two rows from five rooms on, with at most six columns, so twelve rooms sit six on top
  and six below. The Clear button drops the whole wall in one click. Every tile plays
  its own audio: hovering the speaker opens a draggable volume slider, clicking the
  speaker mutes or restores the previous level, and each room keeps its own level.
  The chosen members, layout, volume and dragged row / column sizes are restored next
  time.
* The single-room player and the mini player use the same hover volume slider: hover
  the speaker to drag the level, click it to mute or restore the last level.
