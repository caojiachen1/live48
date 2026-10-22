#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod config;
mod download;
mod events;
mod icons;
mod images;
mod mini;
mod models;
mod perf;
mod player;
mod theme;
mod ui;
mod util;
mod vlc;
#[cfg(windows)]
mod win;

use std::time::{Duration, Instant};

fn main() -> eframe::Result {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn"),
    )
    .try_init();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--apitest") {
        std::process::exit(apitest());
    }
    if let Some(pos) = args.iter().position(|a| a == "--selftest") {
        let Some(target) = args.get(pos + 1) else {
            eprintln!("用法: live48-egui --selftest <视频文件或流地址>");
            std::process::exit(2);
        };
        std::process::exit(selftest(target));
    }
    if let Some(pos) = args.iter().position(|a| a == "--dltest") {
        let (Some(live_id), Some(out), secs) = (
            args.get(pos + 1),
            args.get(pos + 2),
            args.get(pos + 3).and_then(|s| s.parse::<u64>().ok()),
        ) else {
            eprintln!("用法: live48-egui --dltest <liveId> <输出.ts> [秒]");
            std::process::exit(2);
        };
        std::process::exit(dltest(live_id, out, secs.unwrap_or(20)));
    }
    if let Some(pos) = args.iter().position(|a| a == "--recordtest") {
        let (Some(target), Some(out)) = (args.get(pos + 1), args.get(pos + 2)) else {
            eprintln!("用法: live48-egui --recordtest <视频文件或流地址> <输出.ts>");
            std::process::exit(2);
        };
        std::process::exit(recordtest(target, out));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([640.0, 520.0])
            .with_title("live48"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "live48",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

/// Headless check of decode -> libVLC RGBA callback -> frame copy, so the memory
/// video output can be verified without opening a window.
fn selftest(target: &str) -> i32 {
    let engine = match vlc::Engine::load() {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("FAIL engine: {e}");
            return 3;
        }
    };
    let session = match engine.open(target, player::MAX_W, player::MAX_H) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("FAIL open: {e}");
            return 4;
        }
    };

    let mut buf = Vec::new();
    let mut last = 0u64;
    let mut copied = 0usize;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(6) {
        let seq = session.frames().sequence();
        if seq != last {
            last = seq;
            if session.frames().take_rgba(&mut buf) {
                copied += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }

    println!(
        "state={} negotiated={:?} presented={} copied={} rgba_bytes={}",
        session.vlc_state(),
        session.frames().size(),
        session.frames().sequence(),
        copied,
        buf.len()
    );
    if copied > 0 && buf.len() >= 4 {
        println!("SELFTEST PASS");
        0
    } else {
        println!("SELFTEST FAIL");
        5
    }
}

/// Drives the real record downloader for a bounded time so the segment cache, the
/// byte alignment and the cancel path can be checked from a terminal.
fn dltest(live_id: &str, out: &str, secs: u64) -> i32 {
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, mpsc};

    let api = api::Api::new();
    let url = match api.playable_stream_url(live_id, models::Source::Member, None) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("stream url FAILED: {e}");
            return 10;
        }
    };
    println!("url = {}", url.chars().take(90).collect::<String>());

    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    download::start(url, out.to_string(), live_id.to_string(), tx, cancel.clone());

    let t0 = Instant::now();
    let mut last = -100.0;
    let mut outcome = String::from("timeout");
    while t0.elapsed() < Duration::from_secs(secs) {
        if let Ok(events::Event::Download { kind, .. }) = rx.try_recv() {
            match kind {
                events::DownloadEvent::Progress(p) => {
                    if p - last >= 10.0 {
                        println!("progress {p:.1}%");
                        last = p;
                    }
                }
                events::DownloadEvent::Success => {
                    outcome = "success".into();
                    break;
                }
                events::DownloadEvent::Cancelled => {
                    outcome = "cancelled".into();
                    break;
                }
                events::DownloadEvent::Error(e) => {
                    println!("download ERROR: {e}");
                    return 11;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if outcome == "timeout" {
        cancel.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(800));
    }

    let cache = format!("{out}.cache");
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut misaligned = 0usize;
    if let Ok(rd) = std::fs::read_dir(&cache) {
        for entry in rd.flatten() {
            if let Ok(m) = entry.metadata() {
                files += 1;
                bytes += m.len();
                if m.len() % 188 != 0 {
                    misaligned += 1;
                }
            }
        }
    }
    let final_len = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    println!(
        "outcome={outcome} cache_files={files} cache_bytes={bytes} misaligned={misaligned} final_bytes={final_len}"
    );
    let ok = match outcome.as_str() {
        "success" => final_len > 188 && files == 0 && misaligned == 0,
        "cancelled" | "timeout" => files > 0 && misaligned == 0 && final_len == 0,
        _ => false,
    };
    println!(
        "{}",
        if ok {
            "DLTEST PASS"
        } else {
            "DLTEST FAIL (用同一命令再跑一次可验证续传：已存在的分片应被跳过)"
        }
    );
    if ok {
        0
    } else {
        12
    }
}

/// Headless check of the stream-output recorder: the file has to appear and keep
/// growing while VLC is running, which is what the record button relies on.
fn recordtest(target: &str, out: &str) -> i32 {
    let engine = match vlc::Engine::load() {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("FAIL engine: {e}");
            return 3;
        }
    };
    let _ = std::fs::remove_file(out);
    let rec = match engine.record(target, out) {
        Ok(rec) => rec,
        Err(e) => {
            eprintln!("FAIL record: {e}");
            return 4;
        }
    };
    std::thread::sleep(Duration::from_secs(6));
    let a = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    std::thread::sleep(Duration::from_secs(4));
    let b = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    let running = rec.running();
    drop(rec);
    println!(
        "record: running={running} size {a} -> {b} (grew={})",
        b > a
    );
    std::thread::sleep(Duration::from_millis(400));
    let after = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    println!("after stop: {after} bytes at {out}");
    // A live source must still be growing while it runs; a short clip ends before the
    // second sample, which is only acceptable once real packets are on disk.
    let ok = after > 188 && (b > a || !running);
    if ok {
        println!("RECORDTEST PASS");
        0
    } else {
        println!("RECORDTEST FAIL");
        5
    }
}

/// End-to-end check with no window: fetch both lists, resolve a stream address and
/// let libVLC actually decode it, so the whole data + playback path can be verified
/// from a terminal.
fn apitest() -> i32 {
    use std::time::{Duration, Instant};

    let api = api::Api::new();
    let live = match api.live_list() {
        Ok(items) => {
            println!("live list: {} items", items.len());
            items
        }
        Err(e) => {
            eprintln!("live list FAILED: {e}");
            return 10;
        }
    };
    let yesterday = {
        use chrono::{Local, TimeZone};
        let today = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
        Local
            .timestamp_opt(today.and_utc().timestamp(), 0)
            .single()
            .map(|d| d.timestamp_millis() - 86_400_000)
            .unwrap_or(0)
    };
    match api.records_back_to("0", yesterday) {
        Ok((items, next)) => println!("record list: {} items, next={next}", items.len()),
        Err(e) => eprintln!("record list FAILED: {e}"),
    }
    for record in [false, true] {
        match api.open_live_list(record, "0", 0) {
            Ok((items, next)) => println!(
                "open live (record={record}): {} items, next={next}, first={:?}",
                items.len(),
                items.first().map(|i| (i.title.clone(), i.status, i.sub_title.clone()))
            ),
            Err(e) => eprintln!("open live list (record={record}) FAILED: {e}"),
        }
    }

    // Prefer a running live stream; fall back to the newest record so the HLS
    // decode path still gets exercised when nobody is live.
    let records = api.records_back_to("0", yesterday).map(|(v, _)| v).unwrap_or_default();
    if let Some(r) = records.first() {
        match api.room_info(&r.live_id, r.source) {
            Ok(info) => {
                println!(
                    "room {}: room_id={} streams={} msg_file={}",
                    r.live_id,
                    info.room_id,
                    info.streams.len(),
                    info.msg_file_path.is_some()
                );
                if let Some(u) = info.msg_file_path.clone() {
                    match api.danmaku(&u) {
                        Ok(d) => println!(
                            "danmu: {} lines, first={:?}",
                            d.len(),
                            d.first().map(|x| (x.time.clone(), x.nickname.clone(), x.message.chars().take(12).collect::<String>()))
                        ),
                        Err(e) => eprintln!("danmu FAILED: {e}"),
                    }
                }
            }
            Err(e) => eprintln!("room_info FAILED: {e}"),
        }
    }
    let Some(item) = live.first().cloned().or_else(|| records.first().cloned()) else {
        println!("nothing to decode");
        return 0;
    };
    let url = match api.playable_stream_url(&item.live_id, item.source, None) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("stream url FAILED: {e}");
            return 11;
        }
    };
    println!(
        "stream url for {} ({}) = {}",
        item.live_id,
        item.nickname,
        url.chars().take(90).collect::<String>()
    );

    let engine = match vlc::Engine::load() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine FAILED: {e}");
            return 12;
        }
    };
    let ctx = egui::Context::default();
    let entries = vec![player::Entry {
        live_id: item.live_id.clone(),
        title: item.title.clone(),
        nickname: item.nickname.clone(),
        cover: api::resolve_image(&item.cover_path),
        source: item.source,
    }];
    let is_live = !live.is_empty();
    let mut p = match player::Player::open(&engine, &url, entries, 0, is_live, &ctx) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("player open FAILED: {e}");
            return 13;
        }
    };

    let start = Instant::now();
    let seq0 = p.session.frames().sequence();
    let mut pumps = 0u64;
    while start.elapsed() < Duration::from_secs(10) {
        p.pump(&ctx, egui::ViewportId::ROOT);
        pumps += 1;
        std::thread::sleep(Duration::from_millis(8));
    }
    let elapsed = start.elapsed().as_secs_f32().max(0.001);
    let presented = p.session.frames().sequence() - seq0;
    println!(
        "presented {} frames in {elapsed:.1}s ({:.1} fps), texture uploads={} (avg {:.0}us copy), pumps={}, size={:?}, len={}ms, state={}, error={:?}",
        presented,
        presented as f32 / elapsed,
        p.uploads,
        if p.uploads > 0 { p.upload_micros as f32 / p.uploads as f32 } else { 0.0 },
        pumps,
        p.session.frames().size(),
        p.session.length_ms(),
        p.session.vlc_state(),
        p.error
    );
    let mut ok = p.is_started() && presented > 10;

    // The prev/next/refresh buttons swap the session in place, so verify that the
    // old one tears down and the new one delivers frames too.
    let seq_before = p.session.frames().sequence();
    if let Err(e) = p.reattach(&engine, &url, 0) {
        eprintln!("reattach FAILED: {e}");
        ok = false;
    } else {
        let t0 = Instant::now();
        let start_seq = p.session.frames().sequence();
        while t0.elapsed() < Duration::from_secs(6) {
            p.pump(&ctx, egui::ViewportId::ROOT);
            std::thread::sleep(Duration::from_millis(8));
        }
        let after = p.session.frames().sequence();
        println!(
            "after reattach: {} new frames (seq {} -> {}), uploads={}",
            after.saturating_sub(start_seq),
            seq_before,
            after,
            p.uploads
        );
        ok = ok && after.saturating_sub(start_seq) > 10;
    }

    // Seeking is only meaningful for a record, which is what we play when nobody is live.
    if p.session.length_ms() > 0 {
        let target = p.session.length_ms() / 2;
        p.session.set_time_ms(target);
        std::thread::sleep(Duration::from_millis(600));
        let got = p.session.time_ms();
        println!("seek to {target}ms landed at {got}ms");
        ok = ok && (got - target).abs() < 60_000;
    }

    if ok {
        println!("APITEST PASS");
        0
    } else {
        println!("APITEST FAIL");
        14
    }
}
