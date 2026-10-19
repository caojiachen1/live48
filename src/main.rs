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

    // Prefer a running live stream; fall back to the newest record so the HLS
    // decode path still gets exercised when nobody is live.
    let records = api.records_back_to("0", yesterday).map(|(v, _)| v).unwrap_or_default();
    let Some(item) = live.first().cloned().or_else(|| records.first().cloned()) else {
        println!("nothing to decode");
        return 0;
    };
    let url = match api.playable_stream_url(&item.live_id) {
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
