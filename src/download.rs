use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use reqwest::blocking::Response;

use crate::api::Api;
use crate::events::{DownloadEvent, Event};

/// Downloads a record. HLS playlists become one `.ts` file per segment inside a
/// `<target>.cache/` directory, which is what makes an interrupted download resumable:
/// a segment already on disk with a non-zero size is skipped, and "retry" only has to
/// fill the holes. The pieces are then concatenated into the final TS - MPEG-TS is a
/// byte-concatenable format, so no muxer is needed. Everything that is not a playlist
/// streams straight to disk.
pub fn start(
    url: String,
    save_path: String,
    live_id: String,
    tx: Sender<Event>,
    cancelled: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let api = Api::new();
        let send = |kind| {
            let _ = tx.send(Event::Download {
                live_id: live_id.clone(),
                kind,
            });
        };

        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        send(DownloadEvent::Progress(0.0));

        let result = if url.contains(".m3u8") {
            download_hls(&api, &url, &save_path, &tx, &live_id, &cancelled)
        } else {
            download_plain(&api, &url, &save_path, &tx, &live_id, &cancelled)
        };

        match result {
            Ok(true) => send(DownloadEvent::Success),
            Ok(false) => send(DownloadEvent::Cancelled),
            Err(e) => send(DownloadEvent::Error(e)),
        }
    });
}

/// Ok(true) finished, Ok(false) cancelled, Err(message) failed.
type Done = Result<bool, String>;

fn file_size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn segment_name(url: &str) -> String {
    let head = url.split('?').next().unwrap_or(url);
    head.rsplit('/').next().unwrap_or("segment.ts").to_owned()
}

fn stream_to(res: &mut Response, file: &mut std::fs::File, cancelled: &AtomicBool) -> Result<bool, String> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = res.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(true);
        }
        if cancelled.load(Ordering::SeqCst) {
            return Ok(false);
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
}

fn download_hls(
    api: &Api,
    url: &str,
    save_path: &str,
    tx: &Sender<Event>,
    live_id: &str,
    cancelled: &AtomicBool,
) -> Done {
    let text = api.download_text(url)?;
    let base = url::Url::parse(url).map_err(|e| e.to_string())?;
    let parts: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| base.join(l).map(|u| u.to_string()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    if parts.is_empty() {
        return Err("播放列表里没有分片".to_string());
    }

    let cache = std::path::PathBuf::from(format!("{save_path}.cache"));
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;

    let total = parts.len();
    let mut missing = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let done = cache.join(segment_name(part));
        if file_size(&done) > 0 {
            // Already on disk from an earlier run: that is the whole point of the cache.
        } else {
            let tmp = std::path::PathBuf::from(format!("{}.part", done.display()));
            let _ = std::fs::remove_file(&tmp);
            let mut res = api.get(part)?;
            let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            match stream_to(&mut res, &mut file, cancelled) {
                Ok(true) => {
                    let _ = file.flush();
                    drop(file);
                    if file_size(&tmp) == 0 {
                        let _ = std::fs::remove_file(&tmp);
                        missing += 1;
                    } else if let Err(e) = std::fs::rename(&tmp, &done) {
                        return Err(format!("分片落盘失败: {e}"));
                    }
                }
                Ok(false) => {
                    drop(file);
                    let _ = std::fs::remove_file(&tmp);
                    return Ok(false);
                }
                Err(e) => {
                    drop(file);
                    let _ = std::fs::remove_file(&tmp);
                    missing += 1;
                    // One dead fragment should not throw away an hour of downloaded
                    // pieces: keep going and report the holes at the end.
                    let _ = e;
                }
            }
        }
        let _ = tx.send(Event::Download {
            live_id: live_id.to_owned(),
            kind: DownloadEvent::Progress((i + 1) as f64 / total as f64 * 100.0),
        });
    }

    if cancelled.load(Ordering::SeqCst) {
        return Ok(false);
    }
    if missing > 0 {
        return Err(format!("{missing} 个分片下载失败，缓存已保留，可重试续传"));
    }

    let mut out = std::fs::File::create(save_path).map_err(|e| e.to_string())?;
    for part in &parts {
        let src = std::path::PathBuf::from(format!("{}/{}", cache.display(), segment_name(part)));
        let mut f = std::fs::File::open(&src).map_err(|e| format!("分片 {} 缺失: {e}", src.display()))?;
        std::io::copy(&mut f, &mut out).map_err(|e| e.to_string())?;
    }
    let _ = out.flush();
    drop(out);
    let _ = std::fs::remove_dir_all(&cache);
    Ok(true)
}

fn download_plain(
    api: &Api,
    url: &str,
    save_path: &str,
    tx: &Sender<Event>,
    live_id: &str,
    cancelled: &AtomicBool,
) -> Done {
    let total = api.content_length(url);
    let mut res = api.get(url)?;
    let mut file = std::fs::File::create(save_path).map_err(|e| e.to_string())?;
    let mut done = 0u64;
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = res.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if cancelled.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(save_path);
            return Ok(false);
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if total > 0 {
            let _ = tx.send(Event::Download {
                live_id: live_id.to_owned(),
                kind: DownloadEvent::Progress(done as f64 / total as f64 * 100.0),
            });
        }
    }
    let _ = file.flush();
    Ok(true)
}

/// Saves a record's `.lrc` danmaku next to where the video would go.
pub fn save_text(api: &Api, url: &str, path: &str) -> Result<(), String> {
    let body = api.download_text(url)?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}
