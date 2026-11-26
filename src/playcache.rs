//! Read-ahead cache for records.
//!
//! libVLC normally pulls an HLS playlist straight from the CDN with only its own
//! 300ms network cache in front, so one slow segment shows up as a visible hitch and
//! there is nothing to draw on the seek bar. This module fetches the playlist once,
//! downloads segments a little ahead of the playhead into a per-record folder, and
//! serves them back through a loopback HTTP server. Because we own the reads we also
//! know the contiguous buffered end, which is the second rail on the progress bar.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::api::Api;

/// How far ahead of the playhead the workers keep segments on disk. Watched segments
/// are kept: the cache is meant to be cleared from settings, not silently pruned.
const AHEAD_MS: i64 = 240_000;
const WAIT_MS: u64 = 20_000;
/// Parallel segment downloads. A single CDN connection often carries less than the
/// record's bitrate, and then no amount of read-ahead can keep up.
const LANES: usize = 6;

struct Seg {
    url: String,
    path: PathBuf,
    ready: bool,
    failed: bool,
    downloading: bool,
    /// Bytes of the in-progress download and the response length, so the seek bar
    /// can show the buffer growing between two segment boundaries.
    written: u64,
    total: u64,
    dur_ms: i64,
    end_ms: i64,
}

struct State {
    segs: Vec<Seg>,
    stop: bool,
}

pub struct PlayCache {
    state: Mutex<State>,
    wake: Condvar,
    /// The playlist as fetched; tags keep their original order when it is rewritten.
    text: String,
    base: url::Url,
    port: u16,
    total_ms: i64,
    position: AtomicI64,
    /// Bumped on a seek so one reserved lane can start the new segment immediately,
    /// even while the other lanes are still stuck on the old window.
    seek_gen: AtomicU64,
    buffered: AtomicI64,
    stop: AtomicBool,
    closed: AtomicBool,
    dir: PathBuf,
}

impl PlayCache {
    /// `m3u8` is the record's playlist address. Errors when the playlist is not a
    /// plain segment list, and the caller then keeps the direct URL.
    pub fn start(m3u8: &str, live_id: &str) -> Result<Arc<PlayCache>, String> {
        let api = Api::new();
        let text = api.download_text(m3u8)?;
        if !text.contains("#EXT-X-ENDLIST") {
            // A live playlist keeps growing; freezing it would cut the stream short.
            return Err("live playlist".to_owned());
        }
        let base = url::Url::parse(m3u8).map_err(|e| e.to_string())?;
        let segs = parse_playlist(&text, &base)?;
        if segs.is_empty() {
            return Err("播放列表里没有分片".to_owned());
        }

        // Quality variants share a live id but not a segment list, so the folder is
        // keyed by the playlist too; otherwise a 720p fragment could be served for a
        // 1080p record.
        let dir = crate::config::dir().join("playcache").join(format!(
            "{}-{:016x}",
            safe_name(live_id),
            url_hash(m3u8)
        ));
        // Holds the folder in the active list while this cache runs.
        let lease = DirLease::new(&dir);
        let _ = std::fs::create_dir_all(&dir);

        let mut total = 0i64;
        let mut state_segs = Vec::with_capacity(segs.len());
        for (i, (url, dur)) in segs.iter().enumerate() {
            total += *dur;
            let path = dir.join(format!("s{i:05}.ts"));
            let ready = std::fs::metadata(&path)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
            if ready {
                // Left behind when a reader still had the temp file open at close.
                let _ = std::fs::remove_file(path.with_extension("part"));
            }
            state_segs.push(Seg {
                url: url.clone(),
                path,
                ready,
                failed: false,
                downloading: false,
                written: 0,
                total: 0,
                dur_ms: *dur,
                end_ms: total,
            });
        }

        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;

        let cache = Arc::new(PlayCache {
            state: Mutex::new(State {
                segs: state_segs,
                stop: false,
            }),
            wake: Condvar::new(),
            text,
            base,
            port,
            total_ms: total,
            position: AtomicI64::new(0),
            seek_gen: AtomicU64::new(0),
            buffered: AtomicI64::new(0),
            stop: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            dir: dir.clone(),
        });
        lease.keep();
        cache
            .buffered
            .store(buffered_end(&cache), Ordering::Relaxed);

        spawn_server(&cache, listener);
        for lane in 0..LANES {
            spawn_worker(&cache, lane);
        }
        // libVLC gives up on a segment request that takes too long, and on a cold
        // record the first fetch is the slowest one. Have it on disk before handing
        // out the playlist, so playback starts on the first read.
        if !wait_first(&cache, 10_000) {
            cache.close();
            return Err("首段下载超时".to_owned());
        }
        cache
            .buffered
            .store(buffered_end(&cache), Ordering::Relaxed);
        Ok(cache)
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/p.m3u8", self.port)
    }

    pub fn buffered_ms(&self) -> i64 {
        self.buffered.load(Ordering::Relaxed)
    }

    pub fn total_ms(&self) -> i64 {
        self.total_ms
    }

    /// The folder backing this cache; kept after close until settings clears it.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the player is; a jump restarts the download window.
    pub fn note_position(&self, ms: i64) {
        let old = self.position.swap(ms.max(0), Ordering::Relaxed);
        if (old - ms).abs() > 3_000 {
            self.seek_gen.fetch_add(1, Ordering::Relaxed);
            self.wake.notify_all();
        }
    }
    /// Stops the workers and marks the folder free for manual cleanup. The files
    /// themselves are kept: the cache survives closing the player.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut st) = self.state.lock() {
            st.stop = true;
            self.wake.notify_all();
        }
        release(&self.dir);
        if std::env::var("L48_CACHE_DEBUG").is_ok() {
            eprintln!("cache close {}", self.dir.display());
        }
    }
}

impl Drop for PlayCache {
    fn drop(&mut self) {
        self.close();
    }
}

/// Folders with a `PlayCache` alive; manual cache clearing skips them.
fn active_dirs() -> &'static Mutex<HashMap<PathBuf, usize>> {
    static ACTIVE: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register(dir: &Path) {
    let mut map = active_dirs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *map.entry(dir.to_owned()).or_insert(0) += 1;
}

/// Returns true when this was the last cache on that folder.
fn release(dir: &Path) -> bool {
    let mut map = active_dirs()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match map.get_mut(dir) {
        Some(count) if *count > 1 => {
            *count -= 1;
            false
        }
        Some(_) => {
            map.remove(dir);
            true
        }
        None => true,
    }
}

/// Guards a folder during `start`; if setup fails before a `PlayCache` owns it, the
/// guard frees the registration.
struct DirLease {
    dir: PathBuf,
    armed: bool,
}

impl DirLease {
    fn new(dir: &Path) -> Self {
        register(dir);
        Self {
            dir: dir.to_owned(),
            armed: true,
        }
    }

    /// The cache now owns the registration, released from its `close`/`Drop`.
    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for DirLease {
    fn drop(&mut self) {
        if self.armed {
            release(&self.dir);
        }
    }
}

fn safe_name(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn url_hash(url: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    hasher.finish()
}

pub fn root_dir() -> PathBuf {
    crate::config::dir().join("playcache")
}

/// Bytes and record folders currently on disk, for the settings panel.
pub fn disk_usage() -> (u64, usize) {
    let root = root_dir();
    let Ok(rd) = std::fs::read_dir(&root) else {
        return (0, 0);
    };
    let mut bytes = 0u64;
    let mut dirs = 0usize;
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs += 1;
            bytes += dir_size(&path);
        }
    }
    (bytes, dirs)
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|entry| {
            let path = entry.path();
            let meta = entry.metadata().ok();
            if path.is_dir() {
                dir_size(&path)
            } else {
                meta.map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Removes every record folder that no live cache is using. Folders are renamed
/// first so a record opened at the same moment starts with a clean directory.
/// Returns (bytes freed, folders removed, folders kept because they are in use).
pub fn clear_inactive() -> (u64, usize, usize) {
    let root = root_dir();
    let Ok(rd) = std::fs::read_dir(&root) else {
        return (0, 0, 0);
    };
    let mut freed = 0u64;
    let mut removed = 0usize;
    let mut kept = 0usize;
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let target = {
            let map = active_dirs()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if map.get(&path).copied().unwrap_or(0) > 0 {
                kept += 1;
                continue;
            }
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("playcache");
            let trash = path.with_file_name(format!("{name}.trash-{stamp}"));
            match std::fs::rename(&path, &trash) {
                Ok(()) => Some(trash),
                Err(_) => None,
            }
        };
        let Some(trash) = target else { continue };
        let size = dir_size(&trash);
        let mut done = false;
        for _ in 0..10 {
            if std::fs::remove_dir_all(&trash).is_ok() {
                done = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        if done {
            freed += size;
            removed += 1;
        }
    }
    (freed, removed, kept)
}

fn parse_playlist(text: &str, base: &url::Url) -> Result<Vec<(String, i64)>, String> {
    if text.contains("EXT-X-BYTERANGE") {
        // Byte ranges of one file need offset-aware serving; leave those to VLC.
        return Err("byterange playlist".to_owned());
    }
    let mut segs = Vec::new();
    let mut pending: Option<i64> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let dur = rest.split(',').next().unwrap_or("0");
            pending = Some(((dur.parse::<f64>().unwrap_or(0.0) * 1000.0) as i64).max(1));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some(dur) = pending.take() else { continue };
        let abs = base.join(line).map_err(|e| e.to_string())?;
        segs.push((abs.to_string(), dur));
    }
    Ok(segs)
}

/// `#EXT-X-KEY:METHOD=AES-128,URI="k.bin"` (and MAP) only keep working from loopback
/// when those URIs are absolute.
fn absolute_attrs(line: &str, base: &url::Url) -> String {
    let mut out = String::with_capacity(line.len() + 40);
    let mut rest = line;
    while let Some(pos) = rest.find("URI=\"") {
        out.push_str(&rest[..pos + 5]);
        let tail = &rest[pos + 5..];
        let Some(end) = tail.find('"') else {
            out.push_str(tail);
            return out;
        };
        let raw = &tail[..end];
        let abs = base
            .join(raw)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| raw.to_owned());
        out.push_str(&abs);
        out.push('"');
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn playlist_text(cache: &PlayCache) -> String {
    let st = cache.state.lock().unwrap();
    let mut out = String::with_capacity(cache.text.len() + 40);
    let mut pending = false;
    let mut i = 0usize;
    for line in cache.text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with("#EXT-X-DISCONTINUITY") {
            // Timestamps are retimed when segments are served; leaving the tag
            // would make VLC reset its clock at the splice and stall a far seek.
            continue;
        } else if line.starts_with("#EXTINF") {
            pending = true;
            out.push_str(line);
        } else if line.starts_with('#') {
            // KEY and MAP URIs must be absolute once the playlist is served from
            // loopback; every other tag stays exactly where it was.
            out.push_str(&absolute_attrs(line, &cache.base));
        } else if pending && i < st.segs.len() {
            pending = false;
            out.push_str(&format!("http://127.0.0.1:{}/s/{i}.ts", cache.port));
            i += 1;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn spawn_server(cache: &Arc<PlayCache>, listener: TcpListener) {
    let weak = Arc::downgrade(cache);
    std::thread::Builder::new()
        .name("playcache-server".to_owned())
        .spawn(move || loop {
            let Some(c) = weak.upgrade() else { return };
            if c.stop.load(Ordering::Relaxed) {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let w = Arc::downgrade(&c);
                    std::thread::spawn(move || {
                        let Some(c) = w.upgrade() else { return };
                        let _ = serve(stream, &c);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return,
            }
        })
        .ok();
}

fn serve(mut stream: TcpStream, cache: &Arc<PlayCache>) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") || head.len() > 16 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let path = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");
    if std::env::var("L48_CACHE_DEBUG").is_ok() {
        eprintln!("cache req {path}");
    }
    if path.starts_with("/p.m3u8") {
        let body = playlist_text(cache);
        return respond(
            &mut stream,
            "application/vnd.apple.mpegurl",
            body.as_bytes(),
            200,
        );
    }
    if let Some(idx) = path
        .strip_prefix("/s/")
        .and_then(|p| p.split('.').next())
        .and_then(|p| p.parse::<usize>().ok())
    {
        if let Some(bytes) = ready_segment(cache, idx) {
            let range = parse_range(&text, bytes.len());
            return respond_segment(&mut stream, &bytes, range);
        }
        if segment_exists(cache, idx) {
            return stream_segment(&mut stream, cache, idx);
        }
        return respond(&mut stream, "text/plain", b"segment unavailable", 404);
    }
    respond(&mut stream, "text/plain", b"not found", 404)
}

fn seg_state(cache: &Arc<PlayCache>, idx: usize) -> Option<(bool, bool, bool, PathBuf)> {
    let st = cache.state.lock().ok()?;
    let seg = st.segs.get(idx)?;
    Some((
        seg.ready,
        seg.failed,
        seg.downloading,
        seg.path.clone(),
    ))
}

fn seg_expected_ms(cache: &Arc<PlayCache>, idx: usize) -> Option<i64> {
    let st = cache.state.lock().ok()?;
    let seg = st.segs.get(idx)?;
    Some(seg.end_ms - seg.dur_ms)
}

/// Reads the first PES timestamp in a TS file. A record can restart its PTS at an
/// `EXT-X-DISCONTINUITY`; VLC then loses its play clock after a time seek even
/// though the bytes are available.
fn first_pts_in(data: &[u8]) -> Option<i64> {
    let mut off = 0usize;
    while off + 188 <= data.len() {
        if data[off] != 0x47 {
            off += 1;
            continue;
        }
        let b1 = data[off + 1];
        let b3 = data[off + 3];
        let pusi = (b1 & 0x40) != 0;
        let afc = (b3 >> 4) & 3;
        if pusi && (afc & 1) != 0 {
            let mut p = off + 4;
            if (afc & 2) != 0 {
                let afl = data[p] as usize;
                p += 1 + afl;
            }
            if p + 9 <= off + 188 && data[p..p + 3] == [0, 0, 1] {
                let flags = data[p + 7];
                if (flags & 0x80) != 0 {
                    let q = p + 9;
                    if q + 5 <= off + 188 {
                        return Some(read_pts(&data[q..q + 5]));
                    }
                }
            }
        }
        off += 188;
    }
    None
}

fn read_pts(buf: &[u8]) -> i64 {
    (((buf[0] as i64 & 0x0e) << 29)
        | ((buf[1] as i64) << 22)
        | ((buf[2] as i64 & 0xfe) << 14)
        | ((buf[3] as i64) << 7)
        | ((buf[4] as i64) >> 1)) as i64
}

fn write_pts(buf: &mut [u8], value: i64) {
    let v = value as u64 & 0x1_ffff_ffff;
    buf[0] = (buf[0] & 0xf0) | ((((v >> 30) as u8) & 7) << 1) | 1;
    buf[1] = ((v >> 22) & 0xff) as u8;
    buf[2] = ((((v >> 15) as u8) & 0x7f) << 1) | 1;
    buf[3] = ((v >> 7) & 0xff) as u8;
    buf[4] = (((v as u8) & 0x7f) << 1) | 1;
}

fn shift_pcr(buf: &mut [u8], shift: i64) {
    let base = ((buf[0] as i64) << 25)
        | ((buf[1] as i64) << 17)
        | ((buf[2] as i64) << 9)
        | ((buf[3] as i64) << 1)
        | ((buf[4] as i64) >> 7);
    let reserved = (buf[4] >> 1) & 0x3f;
    let ext = (((buf[4] & 1) as u16) << 8) | buf[5] as u16;
    let next = (base + shift).rem_euclid(1 << 33) as u64;
    buf[0] = ((next >> 25) & 0xff) as u8;
    buf[1] = ((next >> 17) & 0xff) as u8;
    buf[2] = ((next >> 9) & 0xff) as u8;
    buf[3] = ((next >> 1) & 0xff) as u8;
    buf[4] = (((next & 1) as u8) << 7) | (reserved << 1) | ((ext >> 8) as u8 & 1);
    buf[5] = (ext & 0xff) as u8;
}

/// Rewrites PTS/DTS and PCR by the same offset so a spliced record looks like one
/// continuous HLS timeline to libVLC. Only the timestamp fields are touched.
fn normalize_ts(data: &mut [u8], shift: i64) {
    if shift == 0 {
        return;
    }
    let mut off = 0usize;
    while off + 188 <= data.len() {
        if data[off] != 0x47 {
            off += 1;
            continue;
        }
        let b1 = data[off + 1];
        let b3 = data[off + 3];
        let pusi = (b1 & 0x40) != 0;
        let afc = (b3 >> 4) & 3;
        let mut payload = off + 4;
        if (afc & 2) != 0 {
            let afl = data[off + 4] as usize;
            let af = off + 5;
            if afl > 0 && af < off + 188 {
                let flags = data[af];
                if (flags & 0x10) != 0 && af + 7 <= off + 188 {
                    shift_pcr(&mut data[af + 1..af + 7], shift);
                }
            }
            payload = off + 5 + afl;
        }
        if pusi && (afc & 1) != 0 && payload + 9 <= off + 188
            && data[payload..payload + 3] == [0, 0, 1]
        {
            let flags = data[payload + 7];
            let pts_at = payload + 9;
            if (flags & 0x80) != 0 && pts_at + 5 <= off + 188 {
                let value = read_pts(&data[pts_at..pts_at + 5]) + shift;
                write_pts(&mut data[pts_at..pts_at + 5], value);
            }
            let dts_at = payload + 14;
            if (flags & 0x40) != 0 && dts_at + 5 <= off + 188 {
                let value = read_pts(&data[dts_at..dts_at + 5]) + shift;
                write_pts(&mut data[dts_at..dts_at + 5], value);
            }
        }
        off += 188;
    }
}

/// Offset for the segment's raw timestamps, or None until the first PES arrives.
fn shift_for(data: &[u8], expected_ms: i64) -> Option<i64> {
    let first = first_pts_in(data)?;
    let diff = expected_ms.max(0) * 90 - first;
    // Segment cuts already start within a keyframe of their EXTINF boundary; only
    // the huge backwards jump of a real discontinuity needs retiming.
    Some(if diff.abs() > 5 * 90_000 { diff } else { 0 })
}

fn first_pts_file(path: &Path) -> Option<i64> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 64 * 1024];
    let mut data = Vec::new();
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pts) = first_pts_in(&data) {
            return Some(pts);
        }
        if data.len() >= 512 * 1024 {
            return None;
        }
    }
}

fn ready_segment(cache: &Arc<PlayCache>, idx: usize) -> Option<Vec<u8>> {
    let (ready, _, _, path) = seg_state(cache, idx)?;
    if !ready {
        return None;
    }
    let mut bytes = std::fs::read(path).ok()?;
    let expected = seg_expected_ms(cache, idx).unwrap_or(0);
    let shift = shift_for(&bytes, expected).unwrap_or(0);
    normalize_ts(&mut bytes, shift);
    Some(bytes)
}

fn segment_exists(cache: &Arc<PlayCache>, idx: usize) -> bool {
    seg_state(cache, idx).is_some()
}

fn shift_for_file(path: &Path, expected_ms: i64) -> Option<i64> {
    let first = first_pts_file(path)?;
    let diff = expected_ms.max(0) * 90 - first;
    Some(if diff.abs() > 5 * 90_000 { diff } else { 0 })
}

fn write_ts_packets(
    stream: &mut TcpStream,
    pending: &mut Vec<u8>,
    shift: i64,
    sent: &mut u64,
) -> std::io::Result<()> {
    let take = pending.len() / 188 * 188;
    if take == 0 {
        return Ok(());
    }
    let mut block = Vec::with_capacity(take);
    block.extend_from_slice(&pending[..take]);
    pending.drain(..take);
    normalize_ts(&mut block, shift);
    write!(stream, "{take:x}\r\n")?;
    stream.write_all(&block)?;
    stream.write_all(b"\r\n")?;
    *sent += take as u64;
    Ok(())
}

/// Serves a segment while the worker is still writing it. libVLC starts decoding as
/// soon as the first bytes arrive; waiting for the whole segment made every seek feel
/// like a cold start on a slow link.
fn stream_segment(stream: &mut TcpStream, cache: &Arc<PlayCache>, idx: usize) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(WAIT_MS);
    let part_of = |path: &Path| path.with_extension("part");

    // Wait for the worker to create the file (or for an older complete copy).
    let path = loop {
        let Some((ready, failed, _, path)) = seg_state(cache, idx) else {
            return respond(stream, "text/plain", b"segment unavailable", 404);
        };
        if ready {
            return match ready_segment(cache, idx) {
                Some(bytes) => respond_segment(stream, &bytes, None),
                None => respond(stream, "text/plain", b"segment unavailable", 404),
            };
        }
        let part = part_of(&path);
        if std::fs::metadata(&part).map(|m| m.len() > 0).unwrap_or(false) {
            break part;
        }
        if failed || cache.stop.load(Ordering::Relaxed) || Instant::now() > deadline {
            return respond(stream, "text/plain", b"segment unavailable", 404);
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // A segment after EXT-X-DISCONTINUITY restarts its PTS. Wait for its first PES
    // so the response can be retimed instead of handing VLC a clock that jumps
    // back to zero.
    let expected = seg_expected_ms(cache, idx).unwrap_or(0);
    let shift_deadline = Instant::now() + Duration::from_millis(2_000);
    let mut shift = 0i64;
    loop {
        if let Some(value) = shift_for_file(&path, expected) {
            shift = value;
            break;
        }
        let (ready, failed, downloading, _current) = match seg_state(cache, idx) {
            Some(v) => v,
            None => break,
        };
        if failed || (!downloading && !ready) || cache.stop.load(Ordering::Relaxed) {
            break;
        }
        if Instant::now() > shift_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut file = std::fs::File::open(&path)?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    )?;
    let mut buf = [0u8; 64 * 1024];
    let mut pending = Vec::new();
    let mut read_bytes = 0u64;
    let mut sent = 0u64;
    loop {
        match file.read(&mut buf) {
            Ok(0) => {
                let (ready, failed, downloading, path) =
                    match seg_state(cache, idx) {
                        Some(v) => v,
                        None => break,
                    };
                if ready {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    if read_bytes >= size {
                        break;
                    }
                }
                if failed || (!downloading && !ready) || cache.stop.load(Ordering::Relaxed) {
                    // A failed or cancelled segment is incomplete; dropping the
                    // connection makes VLC ask again instead of accepting a short file.
                    return Ok(());
                }
                // Reopen so a file that grew (or was renamed on completion) is visible.
                std::thread::sleep(Duration::from_millis(20));
                let current = if path.exists() { path } else { part_of(&path) };
                match std::fs::File::open(&current) {
                    Ok(mut next) => {
                        let _ = next.seek(SeekFrom::Start(read_bytes));
                        file = next;
                    }
                    Err(_) => break,
                }
            }
            Ok(n) => {
                read_bytes += n as u64;
                pending.extend_from_slice(&buf[..n]);
                write_ts_packets(stream, &mut pending, shift, &mut sent)?;
            }
            Err(_) => break,
        }
    }
    if !pending.is_empty() {
        write!(stream, "{:x}\r\n", pending.len())?;
        stream.write_all(&pending)?;
        stream.write_all(b"\r\n")?;
    }
    stream.write_all(b"0\r\n\r\n")?;
    stream.flush()
}

/// `Range: bytes=a-b` from VLC; a range request is answered with a 206 slice so its
/// seek does not have to read the whole segment again.
fn parse_range(head: &str, total: usize) -> Option<(usize, usize)> {
    let line = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))?;
    let spec = line.split_once('=')?.1.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    let start: usize = start.trim().parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if end.trim().is_empty() {
        total - 1
    } else {
        end.trim().parse::<usize>().ok()?.min(total - 1)
    };
    if end < start {
        return None;
    }
    Some((start, end + 1))
}

fn respond_segment(
    stream: &mut TcpStream,
    bytes: &[u8],
    range: Option<(usize, usize)>,
) -> std::io::Result<()> {
    match range {
        Some((start, end)) => {
            let body = &bytes[start..end];
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp2t\r\nContent-Range: bytes {start}-{}-{}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                end - 1,
                bytes.len(),
                body.len()
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(body)?;
        }
        None => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(bytes)?;
        }
    }
    stream.flush()
}

fn respond(stream: &mut TcpStream, mime: &str, body: &[u8], code: u16) -> std::io::Result<()> {
    let reason = if code == 200 { "OK" } else { "Not Found" };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn wait_first(cache: &Arc<PlayCache>, ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    let Ok(mut st) = cache.state.lock() else {
        return false;
    };
    loop {
        if let Some(seg) = st.segs.first() {
            if seg.ready {
                return true;
            }
            let part = seg.path.with_extension("part");
            if std::fs::metadata(&part).map(|m| m.len() > 0).unwrap_or(false) {
                // Progressive serving: the first bytes are enough to start VLC.
                return true;
            }
        }
        if st.stop || Instant::now() > deadline {
            return false;
        }
        let Ok((guard, _)) = cache.wake.wait_timeout(st, Duration::from_millis(50)) else {
            return false;
        };
        st = guard;
    }
}

fn spawn_worker(cache: &Arc<PlayCache>, lane: usize) {
    let weak = Arc::downgrade(cache);
    let urgent = lane == LANES - 1;
    let _ = std::thread::Builder::new()
        .name(format!("playcache-worker-{lane}"))
        .spawn(move || {
            let api = Api::new();
            let mut seen_seek = 0u64;
            let mut urgent_until = Instant::now();
            loop {
                let Some(cache) = weak.upgrade() else { return };
                if cache.stop.load(Ordering::Relaxed) {
                    return;
                }
                if urgent {
                    let gen = cache.seek_gen.load(Ordering::Relaxed);
                    if gen != seen_seek {
                        seen_seek = gen;
                        // Give the seek a short burst on the reserved lane so VLC
                        // can start on the new segment while the others cancel.
                        urgent_until = Instant::now() + Duration::from_secs(8);
                    }
                    if Instant::now() >= urgent_until {
                        let st = match cache.state.lock() {
                            Ok(st) => st,
                            Err(_) => return,
                        };
                        let _ = cache.wake.wait_timeout(st, Duration::from_millis(150));
                        continue;
                    }
                }
                let pos = cache.position.load(Ordering::Relaxed);
                let (start, end) = window(&cache, pos);
                let pick = {
                    let mut st = match cache.state.lock() {
                        Ok(st) => st,
                        Err(_) => return,
                    };
                    let idx = (start..end).find(|&i| {
                        let seg = &st.segs[i];
                        !seg.ready && !seg.downloading
                    });
                    idx.map(|i| {
                        st.segs[i].downloading = true;
                        st.segs[i].written = 0;
                        st.segs[i].total = 0;
                        (i, st.segs[i].url.clone(), st.segs[i].path.clone())
                    })
                };
                let Some((i, url, path)) = pick else {
                    let st = match cache.state.lock() {
                        Ok(st) => st,
                        Err(_) => return,
                    };
                    let _ = cache.wake.wait_timeout(st, Duration::from_millis(150));
                    continue;
                };
                let outcome = fetch(&api, &url, &path, &cache, i);
                if let Ok(mut st) = cache.state.lock() {
                    st.segs[i].downloading = false;
                    match outcome {
                        FetchEnd::Done => {
                            st.segs[i].ready = true;
                            st.segs[i].failed = false;
                        }
                        FetchEnd::Failed => {
                            st.segs[i].ready = false;
                            st.segs[i].failed = true;
                            st.segs[i].written = 0;
                            st.segs[i].total = 0;
                        }
                        // A seek pulled the playhead away; the file is discarded
                        // and this lane picks a segment from the new window.
                        FetchEnd::Cancel => {
                            st.segs[i].written = 0;
                            st.segs[i].total = 0;
                        }
                    }
                }
                cache.wake.notify_all();
                cache
                    .buffered
                    .store(buffered_end(&cache), Ordering::Relaxed);
            }
        });
}

fn window(cache: &PlayCache, pos: i64) -> (usize, usize) {
    let st = cache.state.lock().unwrap();
    let n = st.segs.len();
    let start = st
        .segs
        .iter()
        .position(|s| s.end_ms > pos)
        .unwrap_or(n.saturating_sub(1));
    let end = st
        .segs
        .iter()
        .position(|s| s.end_ms >= pos + AHEAD_MS)
        .map(|i| i + 1)
        .unwrap_or(n)
        .max(start + 1);
    (start, end.min(n))
}

fn buffered_end(cache: &PlayCache) -> i64 {
    let st = cache.state.lock().unwrap();
    let pos = cache.position.load(Ordering::Relaxed);
    let Some(i) = st.segs.iter().position(|s| s.end_ms > pos) else {
        return cache.total_ms;
    };
    let start_of = st.segs[i].end_ms - st.segs[i].dur_ms;
    let part = |seg: &Seg| {
        if seg.total > 0 {
            seg.dur_ms * (seg.written.min(seg.total) as i64) / seg.total as i64
        } else {
            0
        }
    };
    if !st.segs[i].ready {
        return start_of + part(&st.segs[i]);
    }
    let mut end = st.segs[i].end_ms;
    for s in &st.segs[i + 1..] {
        if s.ready {
            end = s.end_ms;
        } else {
            // The worker is still writing this segment; expose the downloaded
            // fraction so the cache rail creeps forward instead of jumping every 6s.
            end += part(s);
            break;
        }
    }
    end
}

enum FetchEnd {
    Done,
    Failed,
    Cancel,
}

fn fetch(api: &Api, url: &str, path: &Path, cache: &PlayCache, idx: usize) -> FetchEnd {
    let mut res = match api.get(url) {
        Ok(res) => res,
        Err(_) => return FetchEnd::Failed,
    };
    if !res.status().is_success() {
        return FetchEnd::Failed;
    }
    let expected = res.content_length().unwrap_or(0);
    if let Ok(mut st) = cache.state.lock() {
        if let Some(seg) = st.segs.get_mut(idx) {
            seg.total = expected;
            seg.written = 0;
        }
    }
    let tmp = path.with_extension("part");
    let mut file = match std::fs::File::create(&tmp) {
        Ok(file) => file,
        Err(_) => return FetchEnd::Failed,
    };
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0usize;
    let mut seen = cache.position.load(Ordering::Relaxed);
    let mut checked = Instant::now();
    loop {
        if cache.stop.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return FetchEnd::Cancel;
        }
        // A seek moves the playhead much faster than wall time; a slow download
        // moves it at about 1x, and a pause does not move it at all.
        let now = cache.position.load(Ordering::Relaxed);
        let wall = checked.elapsed().as_millis() as i64;
        let moved = now - seen;
        if moved.abs() > 2_000 && (moved - wall).abs() > 12_000 {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return FetchEnd::Cancel;
        }
        seen = now;
        checked = Instant::now();
        let n = match res.read(&mut buf) {
            Ok(n) => n,
            Err(_) => {
                drop(file);
                let _ = std::fs::remove_file(&tmp);
                return FetchEnd::Failed;
            }
        };
        if n == 0 {
            break;
        }
        total += n;
        if file.write_all(&buf[..n]).is_err() {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return FetchEnd::Failed;
        }
        if let Ok(mut st) = cache.state.lock() {
            if let Some(seg) = st.segs.get_mut(idx) {
                seg.written = total as u64;
            }
        }
        cache
            .buffered
            .store(buffered_end(cache), Ordering::Relaxed);
    }
    drop(file);
    if total == 0 {
        let _ = std::fs::remove_file(&tmp);
        return FetchEnd::Failed;
    }
    match std::fs::copy(&tmp, path) {
        Ok(_) => {
            // A reader may still hold the `.part` handle; Windows can refuse the
            // delete, so startup also sweeps stale parts.
            let _ = std::fs::remove_file(&tmp);
            FetchEnd::Done
        }
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            FetchEnd::Failed
        }
    }
}
