use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::api::Api;
use crate::events::{DownloadEvent, Event};

/// Downloads a record: HLS playlists are de-multiplexed into one contiguous TS file,
/// everything else is streamed straight to disk. Mirrors the original Rust task,
/// including deleting the partial file on cancel.
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

fn download_hls(api: &Api, url: &str, save_path: &str, tx: &Sender<Event>, live_id: &str, cancelled: &AtomicBool) -> Done {
    let text = api.download_text(url)?;
    let base = url::Url::parse(url).map_err(|e| e.to_string())?;
    let parts: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| base.join(l).map(|u| u.to_string()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;

    let mut file = std::fs::File::create(save_path).map_err(|e| e.to_string())?;
    let total = parts.len().max(1);

    for (i, part) in parts.iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(save_path);
            return Ok(false);
        }
        let mut res = api.get(part)?;
        let mut buf = [0u8; 64 * 1024];
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
        }
        let _ = tx.send(Event::Download {
            live_id: live_id.to_owned(),
            kind: DownloadEvent::Progress((i + 1) as f64 / total as f64 * 100.0),
        });
    }
    let _ = file.flush();
    Ok(true)
}

fn download_plain(api: &Api, url: &str, save_path: &str, tx: &Sender<Event>, live_id: &str, cancelled: &AtomicBool) -> Done {
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
