use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE, CONTENT_TYPE, HOST, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tauri::Manager;
use tauri_plugin_fs::FsExt;
use url::Url;

// 构造统一的请求头
fn get_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("pocketapi.48.cn"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json;charset=utf-8"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("PocketFans201807/6.0.16 (iPhone; iOS 13.5.1; Scale/2.00)"),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-Hans-AW;q=1"));
    headers.insert(
        "appInfo",
        HeaderValue::from_static(r#"{"vendor":"apple","deviceId":"11111111-1111-1111-1111-111111111111","appVersion":"7.0.4","appBuild":"23011601","osVersion":"16.3.1","osType":"ios","deviceName":"iPhone XR","os":"ios"}"#)
    );
    headers
}

#[tauri::command]
async fn get_live_list() -> Result<Value, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "debug": true,
        "next": "0",
        "groupId": 0,
        "record": false
    });

    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getLiveList")
        .headers(get_headers())
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(json)
}

#[tauri::command]
async fn get_record_list(next: String) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "debug": true,
        "next": next,
        "groupId": 0,
        "record": true
    });

    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getLiveList")
        .headers(get_headers())
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(json)
}

#[tauri::command]
async fn get_live_stream_url(live_id: String) -> Result<String, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "liveId": live_id
    });

    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getLiveOne")
        .headers(get_headers())
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = res.json().await.map_err(|e| e.to_string())?;

    if let Some(path) = json["content"]["playStreamPath"].as_str() {
        Ok(path.to_string())
    } else {
        Err("Failed to parse playStreamPath".to_string())
    }
}

#[tauri::command]
fn open_in_player(url: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // 增加更准确全面的 PotPlayer 路径匹配
        let pot_paths = vec![
            "PotPlayer64",
            "PotPlayerMini64",
            "PotPlayer",
            "PotPlayerMini",
            "C:\\Program Files\\DAUM\\PotPlayer\\PotPlayer64.exe",
            "C:\\Program Files\\DAUM\\PotPlayer\\PotPlayerMini64.exe",
            "C:\\Program Files (x86)\\DAUM\\PotPlayer\\PotPlayer.exe",
            "C:\\Program Files (x86)\\DAUM\\PotPlayer\\PotPlayerMini.exe",
        ];

        let mut launched = false;
        for path in pot_paths {
            if Command::new(path).arg(&url).spawn().is_ok() {
                launched = true;
                break;
            }
        }

        if !launched {
            // 如果没找到，不要用 cmd /c start 因为在遇到带 '&' 等符号的 url 时会被截断，导致参数丢失。
            // 使用 powershell 的 Start-Process 可以确保 url 被整体传递
            let _ = Command::new("powershell")
                .args(&[
                    "-NoProfile",
                    "-WindowStyle",
                    "Hidden",
                    "-Command",
                    &format!("Start-Process '{}'", url.replace("'", "''")),
                ])
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("open").arg(&url).spawn();
    }

    Ok(())
}

#[derive(Clone, serde::Serialize)]
struct DownloadEvent {
    status: String,
    message: String,
    progress: f64,
    live_id: String,
}

/// 存储每个下载任务的取消标志
type CancelMap = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[tauri::command]
async fn download_video(
    app_handle: tauri::AppHandle,
    url: String,
    save_path: String,
    live_id: String,
) -> Result<(), String> {
    let cancel_map: CancelMap = app_handle.state::<CancelMap>().inner().clone();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let mut cancels = cancel_map.lock().map_err(|e| e.to_string())?;
        cancels.insert(live_id.clone(), cancel_flag.clone());
    }

    tauri::async_runtime::spawn(async move {
        // Emit start event
        let _ = app_handle.emit(
            "download-status",
            DownloadEvent {
                status: "start".to_string(),
                message: save_path.clone(),
                progress: 0.0,
                live_id: live_id.clone(),
            },
        );

        match download_video_task(
            app_handle.clone(),
            url.clone(),
            save_path.clone(),
            live_id.clone(),
            cancel_flag.clone(),
        )
        .await
        {
            Ok(_) => {
                // download_video_task emits success/cancelled itself
            }
            Err(e) => {
                let _ = app_handle.emit(
                    "download-status",
                    DownloadEvent {
                        status: "error".to_string(),
                        message: format!("{}: {}", save_path, e),
                        progress: 0.0,
                        live_id: live_id.clone(),
                    },
                );
            }
        }

        // 清理注册表
        if let Ok(mut cancels) = cancel_map.lock() {
            cancels.remove(&live_id);
        }
    });

    Ok(())
}

#[tauri::command]
async fn cancel_download(live_id: String, cancel_map: tauri::State<'_, CancelMap>) -> Result<(), String> {
    let mut cancels = cancel_map.lock().map_err(|e| e.to_string())?;
    if let Some(flag) = cancels.remove(&live_id) {
        flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}

async fn download_video_task(
    app_handle: tauri::AppHandle,
    url: String,
    save_path: String,
    live_id: String,
    cancelled: Arc<AtomicBool>,
) -> Result<(), String> {
    // 检查是否已被取消
    if cancelled.load(Ordering::SeqCst) {
        return Ok(());
    }

    let is_content_uri = save_path.starts_with("content://");

    if is_content_uri {
        // Android content:// URI: use tauri-plugin-fs with spawn_blocking
        download_via_content_resolver(&app_handle, &url, &save_path, &live_id, &cancelled).await
    } else {
        // Desktop/regular path: use std::fs directly
        download_via_std_fs(&app_handle, &url, &save_path, &live_id, &cancelled).await
    }
}

async fn download_via_content_resolver(
    app_handle: &tauri::AppHandle,
    url: &str,
    save_path: &str,
    live_id: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let client = reqwest::Client::new();

    if url.contains(".m3u8") {
        let res = client.get(url).send().await.map_err(|e| e.to_string())?;
        let text = res.text().await.map_err(|e| e.to_string())?;
        let base_url = Url::parse(url).map_err(|e| e.to_string())?;

        let lines: Vec<&str> = text
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .collect();

        let total_parts = lines.len();
        let app_handle = app_handle.clone();
        let save_path = save_path.to_string();
        let live_id = live_id.to_string();
        let cancelled = cancelled.clone();
        let lines_data: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

        // Use spawn_blocking for content:// URI operations
        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let fs = app_handle.fs();
            let mut options = tauri_plugin_fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            // Parse content:// URI as Url for tauri-plugin-fs
            let file_path = url::Url::parse(&save_path).map_err(|e| e.to_string())?;
            let mut file = fs.open(
                file_path,
                options,
            )
            .map_err(|e| e.to_string())?;

            let sync_client = reqwest::blocking::Client::new();

            for (index, line) in lines_data.iter().enumerate() {
                if cancelled.load(Ordering::SeqCst) {
                    let _ = app_handle.emit(
                        "download-status",
                        DownloadEvent {
                            status: "cancelled".to_string(),
                            message: save_path.clone(),
                            progress: 0.0,
                            live_id: live_id.clone(),
                        },
                    );
                    return Ok(());
                }

                let ts_url = base_url.join(line).map_err(|e| e.to_string())?;
                let mut chunk_res = sync_client
                    .get(ts_url)
                    .send()
                    .map_err(|e| e.to_string())?;

                let mut buf = [0u8; 8192];
                loop {
                    let n = chunk_res.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    if cancelled.load(Ordering::SeqCst) {
                        let _ = app_handle.emit(
                            "download-status",
                            DownloadEvent {
                                status: "cancelled".to_string(),
                                message: save_path.clone(),
                                progress: 0.0,
                                live_id: live_id.clone(),
                            },
                        );
                        return Ok(());
                    }
                    file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                }

                let progress = (index as f64 + 1.0) / (total_parts as f64) * 100.0;
                let _ = app_handle.emit(
                    "download-status",
                    DownloadEvent {
                        status: "progress".to_string(),
                        message: save_path.clone(),
                        progress,
                        live_id: live_id.clone(),
                    },
                );
            }

            Ok(())
        })
        .await
        .map_err(|e| e.to_string())??;
    } else {
        let app_handle = app_handle.clone();
        let save_path = save_path.to_string();
        let live_id = live_id.to_string();
        let cancelled = cancelled.clone();
        let url = url.to_string();

        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let fs = app_handle.fs();
            let mut options = tauri_plugin_fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            // Parse content:// URI as Url for tauri-plugin-fs
            let file_path = url::Url::parse(&save_path).map_err(|e| e.to_string())?;
            let mut file = fs.open(
                file_path,
                options,
            )
            .map_err(|e| e.to_string())?;

            let sync_client = reqwest::blocking::Client::new();
            let mut res = sync_client.get(&url).send().map_err(|e| e.to_string())?;
            let total_size = res.content_length().unwrap_or(0);
            let mut downloaded: u64 = 0;

            let mut buf = [0u8; 8192];
            loop {
                let n = res.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                if cancelled.load(Ordering::SeqCst) {
                    let _ = app_handle.emit(
                        "download-status",
                        DownloadEvent {
                            status: "cancelled".to_string(),
                            message: save_path.clone(),
                            progress: 0.0,
                            live_id: live_id.clone(),
                        },
                    );
                    return Ok(());
                }

                file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                downloaded += n as u64;
                if total_size > 0 {
                    let progress = (downloaded as f64 / total_size as f64) * 100.0;
                    let _ = app_handle.emit(
                        "download-status",
                        DownloadEvent {
                            status: "progress".to_string(),
                            message: save_path.clone(),
                            progress,
                            live_id: live_id.clone(),
                        },
                    );
                }
            }

            Ok(())
        })
        .await
        .map_err(|e| e.to_string())??;
    }

    // 下载成功
    let _ = app_handle.emit(
        "download-status",
        DownloadEvent {
            status: "success".to_string(),
            message: save_path.to_string(),
            progress: 100.0,
            live_id: live_id.to_string(),
        },
    );

    Ok(())
}

async fn download_via_std_fs(
    app_handle: &tauri::AppHandle,
    url: &str,
    save_path: &str,
    live_id: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let client = reqwest::Client::new();

    if url.contains(".m3u8") {
        let res = client.get(url).send().await.map_err(|e| e.to_string())?;
        let text = res.text().await.map_err(|e| e.to_string())?;
        let base_url = Url::parse(url).map_err(|e| e.to_string())?;

        let mut file = std::fs::File::create(save_path).map_err(|e| e.to_string())?;

        let lines: Vec<&str> = text
            .lines()
            .filter(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#')
            })
            .collect();

        let total_parts = lines.len();

        for (index, line) in lines.iter().enumerate() {
            if cancelled.load(Ordering::SeqCst) {
                drop(file);
                let _ = std::fs::remove_file(save_path);
                let _ = app_handle.emit(
                    "download-status",
                    DownloadEvent {
                        status: "cancelled".to_string(),
                        message: save_path.to_string(),
                        progress: 0.0,
                        live_id: live_id.to_string(),
                    },
                );
                return Ok(());
            }

            let ts_url = base_url.join(line).map_err(|e| e.to_string())?;
            let mut chunk_res = client
                .get(ts_url)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            while let Some(chunk) = chunk_res.chunk().await.map_err(|e| e.to_string())? {
                if cancelled.load(Ordering::SeqCst) {
                    drop(file);
                    let _ = std::fs::remove_file(save_path);
                    let _ = app_handle.emit(
                        "download-status",
                        DownloadEvent {
                            status: "cancelled".to_string(),
                            message: save_path.to_string(),
                            progress: 0.0,
                            live_id: live_id.to_string(),
                        },
                    );
                    return Ok(());
                }
                file.write_all(&chunk).map_err(|e| e.to_string())?;
            }

            let progress = (index as f64 + 1.0) / (total_parts as f64) * 100.0;
            let _ = app_handle.emit(
                "download-status",
                DownloadEvent {
                    status: "progress".to_string(),
                    message: save_path.to_string(),
                    progress,
                    live_id: live_id.to_string(),
                },
            );
        }
    } else {
        let mut res = client.get(url).send().await.map_err(|e| e.to_string())?;
        let mut file = std::fs::File::create(save_path).map_err(|e| e.to_string())?;
        let total_size = res.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;

        while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
            if cancelled.load(Ordering::SeqCst) {
                drop(file);
                let _ = std::fs::remove_file(save_path);
                let _ = app_handle.emit(
                    "download-status",
                    DownloadEvent {
                        status: "cancelled".to_string(),
                        message: save_path.to_string(),
                        progress: 0.0,
                        live_id: live_id.to_string(),
                    },
                );
                return Ok(());
            }

            file.write_all(&chunk).map_err(|e| e.to_string())?;
            downloaded += chunk.len() as u64;
            if total_size > 0 {
                let progress = (downloaded as f64 / total_size as f64) * 100.0;
                let _ = app_handle.emit(
                    "download-status",
                    DownloadEvent {
                        status: "progress".to_string(),
                        message: save_path.to_string(),
                        progress,
                        live_id: live_id.to_string(),
                    },
                );
            }
        }
    }

    // 下载成功
    let _ = app_handle.emit(
        "download-status",
        DownloadEvent {
            status: "success".to_string(),
            message: save_path.to_string(),
            progress: 100.0,
            live_id: live_id.to_string(),
        },
    );

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .manage::<CancelMap>(Arc::new(Mutex::new(HashMap::new())))
        .invoke_handler(tauri::generate_handler![
            get_live_list,
            get_record_list,
            get_live_stream_url,
            open_in_player,
            download_video,
            cancel_download
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}