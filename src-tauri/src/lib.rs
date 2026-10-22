use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE, CONTENT_TYPE, HOST, USER_AGENT};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tauri::Manager;
use tauri::WebviewWindowBuilder;
use tauri_plugin_fs::FsExt;
use url::Url;

mod vlc_player;
use vlc_player::{FileRecorder, Libvlc, VlcStreamer, VlcStreamState};

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

/// 口袋48录播的 m3u8 与 ts 分片要用这个 UA 请求，否则 CDN 直接拒绝。
/// 不能复用 get_headers()：那里的 Host 是写死的 pocketapi.48.cn，
/// CDN 请求的 Host 必须由 URL 自动推导。
const ENGINE_USER_AGENT: &str = "SNH48 ENGINE";

fn cdn_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(ENGINE_USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn cdn_blocking_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(ENGINE_USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
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

/// 按成员查历史录播。
/// 服务端有个坑：next 传 "0" 时带上 userId 是查不到数据的，要先拉一次不带 userId 的列表，
/// 用第一条的 liveId 当 next 再查；如果那条正好就是目标成员，还要手动塞回结果头部。
#[tauri::command]
async fn get_user_record_list(
    user_id: String,
    next: String,
    record: bool,
    group_id: Option<i64>,
) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let post_list = |body: Value| {
        let client = client.clone();
        async move {
            let res = client
                .post("https://pocketapi.48.cn/live/api/v1/live/getLiveList")
                .headers(get_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            res.json::<Value>().await.map_err(|e| e.to_string())
        }
    };

    let mut real_next = next.clone();
    let mut first_item: Option<Value> = None;

    if next == "0" || next.is_empty() {
        let pre = post_list(serde_json::json!({
            "debug": false,
            "next": "0",
            "groupId": group_id.unwrap_or(0),
            "record": record
        }))
        .await?;
        if let Some(first) = pre["content"]["liveList"].as_array().and_then(|l| l.first()) {
            if let Some(live_id) = first["liveId"].as_str() {
                real_next = live_id.to_string();
            }
            if first["userInfo"]["userId"].as_str() == Some(user_id.as_str()) {
                first_item = Some(first.clone());
            }
        }
    }

    // userId 查询时不能同时带 groupId，否则服务端返回空列表
    let mut body = serde_json::json!({
        "debug": false,
        "next": real_next,
        "record": record
    });
    match user_id.parse::<i64>() {
        Ok(id) => body["userId"] = serde_json::json!(id),
        Err(_) => body["userId"] = serde_json::json!(user_id),
    }

    let mut json = post_list(body).await?;
    if let Some(first) = first_item {
        // 第一条正好是目标成员时，它被 next 过滤掉了，要补回列表头部
        if let Some(list) = json["content"]["liveList"].as_array_mut() {
            let already_there = list.first().and_then(|f| f["liveId"].as_str()) == first["liveId"].as_str();
            if !already_there {
                list.insert(0, first);
            }
        }
    }

    Ok(json)
}

/// 公演（剧场直播）列表：独立于成员直播的一套接口，next 传数字
#[tauri::command]
async fn get_open_live_list(next: String, record: bool, group_id: Option<i64>) -> Result<Value, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "debug": false,
        "groupId": group_id.unwrap_or(0),
        "next": next.parse::<i64>().unwrap_or(0),
        "record": record
    });

    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getOpenLiveList")
        .headers(get_headers())
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = res.json().await.map_err(|e| e.to_string())?;
    Ok(json)
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenLiveStream {
    stream_name: String,
    stream_type: i64,
    stream_path: String,
    vip_show: bool,
}

/// 公演流地址：走 getOpenLiveOne（不是 getLiveOne），返回带清晰度的多个流
#[tauri::command]
async fn get_open_live_streams(live_id: String) -> Result<Vec<OpenLiveStream>, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "liveId": live_id });

    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getOpenLiveOne")
        .headers(get_headers())
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = res.json().await.map_err(|e| e.to_string())?;
    let streams = json["content"]["playStreams"]
        .as_array()
        .ok_or_else(|| "公演没有可用的流地址".to_string())?;

    let list: Vec<OpenLiveStream> = streams
        .iter()
        .map(|s| OpenLiveStream {
            stream_name: s["streamName"].as_str().unwrap_or_default().to_string(),
            stream_type: s["streamType"].as_i64().unwrap_or(0),
            stream_path: s["streamPath"].as_str().unwrap_or_default().to_string(),
            vip_show: s["vipShow"].as_bool().unwrap_or(false),
        })
        .filter(|s| !s.stream_path.is_empty())
        .collect();

    if list.is_empty() {
        return Err("公演没有可用的流地址".to_string());
    }
    Ok(list)
}

/// 获取直播/录播流地址（从 getLiveOne 接口）
/// 可能返回 rtmp:// 或 https://...m3u8
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

/// 获取直播流地址（使用 getlivestream 接口，streamType=3）
/// 这个接口返回的通常是 HTTPS m3u8 地址，更适合浏览器播放
#[tauri::command]
async fn get_live_stream_url_v2(live_id: String) -> Result<String, String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "liveId": live_id,
        "streamType": 3
    });

    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getlivestream")
        .headers(get_headers())
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = res.json().await.map_err(|e| e.to_string())?;

    // getlivestream 接口的 content 字段直接是字符串地址
    if let Some(path) = json["content"].as_str() {
        Ok(path.to_string())
    } else {
        // 如果 getlivestream 失败，回退到 getLiveOne
        get_live_stream_url(live_id).await
    }
}

/// 尝试获取可播放的流地址（优先 HTTPS m3u8，避免 RTMP）
/// 如果 getLiveOne 返回 RTMP，尝试用 getlivestream 拿 HTTPS 地址
/// 如果返回的 playStreamPath 是 RTMP，尝试从 playStreams 中取 HTTPS 流
#[tauri::command]
async fn get_playable_stream_url(live_id: String) -> Result<String, String> {
    let client = reqwest::Client::new();

    // 方法1：先尝试 getOne 看 playStreams 中是否有 HTTPS 流
    let body1 = serde_json::json!({ "liveId": live_id });
    let res1 = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getLiveOne")
        .headers(get_headers())
        .json(&body1)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let json1: Value = res1.json().await.map_err(|e| e.to_string())?;

    // 先看 playStreams（公演直播有多个清晰度）
    if let Some(streams) = json1["content"]["playStreams"].as_array() {
        // 优先选 streamType=3（超清），其次 streamType=1（标清）
        for st in &[3, 1, 2] {
            for stream in streams {
                if stream["streamType"].as_i64() == Some(*st) {
                    if let Some(path) = stream["streamPath"].as_str() {
                        if !path.starts_with("rtmp://") && !path.starts_with("rtmps://") {
                            return Ok(path.to_string());
                        }
                    }
                }
            }
        }
        // playStreams 中全是 RTMP，fallback 继续
    }

    // 看 playStreamPath
    if let Some(path) = json1["content"]["playStreamPath"].as_str() {
        if !path.starts_with("rtmp://") && !path.starts_with("rtmps://") {
            return Ok(path.to_string());
        }
    }

    // 方法2：playStreamPath 是 RTMP，尝试 getlivestream
    let body2 = serde_json::json!({
        "liveId": live_id,
        "streamType": 3
    });
    let res2 = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getlivestream")
        .headers(get_headers())
        .json(&body2)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let json2: Value = res2.json().await.map_err(|e| e.to_string())?;

    if let Some(path) = json2["content"].as_str() {
        if !path.starts_with("rtmp://") && !path.starts_with("rtmps://") {
            return Ok(path.to_string());
        }
    }

    // 所有接口都返回 RTMP，无法在浏览器中播放
    Err("所有可用流地址均为 RTMP 协议，浏览器无法直接播放".to_string())
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DanmuItem {
    time: String,
    current_time: f64,
    nickname: String,
    message: String,
}

/// "[00:05:43.453]" -> 秒
fn parse_danmu_time(time: &str) -> Option<f64> {
    let mut hms = time.split(':');
    let hours = hms.next()?.parse::<f64>().ok()?;
    let minutes = hms.next()?.parse::<f64>().ok()?;
    let mut sec_ms = hms.next()?.split('.');
    let seconds = sec_ms.next()?.parse::<f64>().ok()?;
    let millis = sec_ms.next().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
    Some(hours * 3600.0 + minutes * 60.0 + seconds + millis / 1000.0)
}

/// 弹幕文件每行形如 `[HH:MM:SS.mmm]昵称<TAB>弹幕内容`
fn parse_danmu_text(text: &str) -> Vec<DanmuItem> {
    let mut list: Vec<DanmuItem> = Vec::new();

    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix('[') else { continue };
        let Some((time, no_time)) = rest.split_once(']') else { continue };
        let Some(current_time) = parse_danmu_time(time) else { continue };

        let mut parts = no_time.split('\t');
        list.push(DanmuItem {
            time: time.to_string(),
            current_time,
            nickname: parts.next().unwrap_or_default().to_string(),
            message: parts.collect::<Vec<&str>>().join(""),
        });
    }

    list.sort_by(|a, b| {
        a.current_time
            .partial_cmp(&b.current_time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    list
}

async fn get_msg_file_path(live_id: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getLiveOne")
        .headers(get_headers())
        .json(&serde_json::json!({ "liveId": live_id }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = res.json().await.map_err(|e| e.to_string())?;
    let path = json["content"]["msgFilePath"].as_str().unwrap_or_default();
    if path.is_empty() {
        return Ok(String::new());
    }
    if path.starts_with("http") {
        return Ok(path.to_string());
    }
    Ok(format!("https://pocketapi.48.cn{}", path))
}

/// 录播弹幕：msgFilePath 为空或取不到都当作"没有弹幕"，不报错
#[tauri::command]
async fn get_record_danmu(live_id: String) -> Result<Vec<DanmuItem>, String> {
    let file_path = match get_msg_file_path(&live_id).await {
        Ok(path) => path,
        Err(_) => return Ok(Vec::new()),
    };
    if file_path.is_empty() {
        return Ok(Vec::new());
    }

    let text = match reqwest::Client::new().get(&file_path).send().await {
        Ok(res) => match res.text().await {
            Ok(text) => text,
            Err(_) => return Ok(Vec::new()),
        },
        Err(_) => return Ok(Vec::new()),
    };

    Ok(parse_danmu_text(&text))
}

/// 下载弹幕文件到本地（文件很小，同步下完即可，不进下载任务队列）
#[tauri::command]
async fn download_danmu_file(live_id: String, save_path: String) -> Result<String, String> {
    let file_path = get_msg_file_path(&live_id).await?;
    if file_path.is_empty() {
        return Err("弹幕文件不存在".to_string());
    }

    let res = reqwest::Client::new()
        .get(&file_path)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;

    std::fs::write(&save_path, &bytes).map_err(|e| e.to_string())?;
    Ok(save_path)
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

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(&url).spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(&url).spawn();
    }

    Ok(())
}

#[tauri::command]
fn open_in_vlc(url: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let vlc_paths = vec![
            "vlc",
            "C:\\Program Files\\VideoLAN\\VLC\\vlc.exe",
            "C:\\Program Files (x86)\\VideoLAN\\VLC\\vlc.exe",
        ];
        for path in vlc_paths {
            if Command::new(path).arg(&url).spawn().is_ok() {
                return Ok(());
            }
        }
        return Err("未找到 VLC 播放器，请先安装 VLC".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        if Command::new("open").args(&["-a", "VLC", &url]).spawn().is_ok() {
            return Ok(());
        }
        return Err("未找到 VLC 播放器".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        if Command::new("vlc").arg(&url).spawn().is_ok() {
            return Ok(());
        }
        return Err("未找到 VLC 播放器".to_string());
    }
}

/// 启动 VLC 后台转码：将 RTMP 流实时转为 HTTP MPEG-TS 流
/// 返回本地 HTTP URL，前端用 hls.js / <video> 在当前窗口播放
#[tauri::command]
async fn vlc_play(
    state: tauri::State<'_, VlcStreamState>,
    url: String,
) -> Result<String, String> {
    let mut streamer = state.inner().0.lock().unwrap_or_else(|p| p.into_inner());
    let local_url = streamer.start_stream(&url).map_err(|e| {
        eprintln!("[vlc_play] 完整错误: {}", e);
        e
    })?;
    Ok(local_url)
}

/// 停止 VLC 后台转码
#[tauri::command]
async fn vlc_stop(
    state: tauri::State<'_, VlcStreamState>,
) -> Result<(), String> {
    let mut streamer = state.inner().0.lock().unwrap_or_else(|p| p.into_inner());
    streamer.stop_stream();
    Ok(())
}

/// 检查 VLC 是否可用（尝试加载 libvlc）
#[tauri::command]
async fn check_vlc_installed() -> bool {
    Libvlc::load().is_ok()
}

/// 检查 VLC 完整性（文件存在 + 可加载）
#[tauri::command]
async fn check_vlc_integrity() -> Result<String, String> {
    match vlc_player::check_vlc_integrity() {
        Ok(()) => Ok("ok".to_string()),
        Err(e) => Err(e),
    }
}

#[derive(Clone, serde::Serialize)]
struct VlcDownloadProgress {
    progress: f64,
    message: String,
}

/// 下载 VLC 引擎（从 NuGet），下载完成后自动做完整性检查
#[tauri::command]
async fn download_vlc(app_handle: tauri::AppHandle) -> Result<(), String> {
    let app_clone = app_handle.clone();
    
    // 通知开始
    let _ = app_clone.emit(
        "vlc-download-progress",
        VlcDownloadProgress {
            progress: 0.0,
            message: "开始下载 VLC 引擎...".to_string(),
        },
    );

    // 在独立线程中执行阻塞的下载和解压操作
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let target_dir = vlc_player::bundled_dir().ok_or("无法确定 VLC 目录位置")?;
            
            let url = "https://www.nuget.org/api/v2/package/VideoLAN.LibVLC.Windows/3.0.21";
            
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
            
            let _ = app_clone.emit(
                "vlc-download-progress",
                VlcDownloadProgress {
                    progress: 5.0,
                    message: "正在连接下载服务器...".to_string(),
                },
            );

            let resp = client.get(url).send()
                .map_err(|e| format!("下载 VLC 引擎失败: {}", e))?;
            
            let total_size = resp.content_length().unwrap_or(0);
            let _ = app_clone.emit(
                "vlc-download-progress",
                VlcDownloadProgress {
                    progress: 10.0,
                    message: format!("正在下载 (共 {:.1} MB)...", total_size as f64 / 1_048_576.0),
                },
            );

            // 将响应体流式读入内存
            let mut downloaded: u64 = 0;
            let mut bytes = Vec::with_capacity(total_size as usize);
            let mut buf = [0u8; 65536];
            let mut stream = resp;
            
            loop {
                let n = stream.read(&mut buf).map_err(|e| format!("读取数据失败: {}", e))?;
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                downloaded += n as u64;
                
                if total_size > 0 {
                    let pct = 10.0 + (downloaded as f64 / total_size as f64) * 80.0;
                    let _ = app_clone.emit(
                        "vlc-download-progress",
                        VlcDownloadProgress {
                            progress: pct,
                            message: format!("下载中... {:.1}%", pct),
                        },
                    );
                }
            }

            let _ = app_clone.emit(
                "vlc-download-progress",
                VlcDownloadProgress {
                    progress: 90.0,
                    message: "正在解压...".to_string(),
                },
            );

            // 解压到目标目录
            let cursor = std::io::Cursor::new(bytes);
            let mut archive = zip::ZipArchive::new(cursor)
                .map_err(|e| format!("解压文件失败: {}", e))?;

            // 找到 libvlc.dll 所在的前缀目录
            let prefix = (0..archive.len())
                .find_map(|i| {
                    let entry = archive.by_index(i).ok()?;
                    let name = entry.name().replace('\\', "/");
                    if name.ends_with("/libvlc.dll") || name.ends_with("/libvlccore.dll") {
                        name.rfind('/').map(|idx| name[..=idx].to_string())
                    } else {
                        None
                    }
                })
                .ok_or("在 NuGet 包中未找到 libvlc.dll")?;

            // 先清空旧目录
            if target_dir.exists() {
                std::fs::remove_dir_all(&target_dir).map_err(|e| e.to_string())?;
            }
            std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;

            let total_files = archive.len();
            for i in 0..total_files {
                let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
                let name = entry.name().replace('\\', "/");

                if !name.starts_with(&prefix) {
                    continue;
                }

                let rel_path = &name[prefix.len()..];
                let target_path = target_dir.join(rel_path);

                if entry.is_dir() {
                    let _ = std::fs::create_dir_all(&target_path);
                } else if entry.is_file() {
                    if let Some(parent) = target_path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut out = std::fs::File::create(&target_path)
                        .map_err(|e| format!("创建文件失败: {}", e))?;
                    std::io::copy(&mut entry, &mut out)
                        .map_err(|e| format!("写入文件失败: {}", e))?;
                }

                if i % 20 == 0 {
                    let extract_pct = 90.0 + (i as f64 / total_files as f64) * 9.0;
                    let _ = app_clone.emit(
                        "vlc-download-progress",
                        VlcDownloadProgress {
                            progress: extract_pct.min(99.0),
                            message: format!("解压中... {} / {}", i, total_files),
                        },
                    );
                }
            }

            let _ = app_clone.emit(
                "vlc-download-progress",
                VlcDownloadProgress {
                    progress: 99.0,
                    message: "正在验证完整性...".to_string(),
                },
            );

            // 完整性检查
            vlc_player::check_vlc_integrity().map_err(|e| format!("完整性检查失败: {}", e))?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                let _ = app_clone.emit(
                    "vlc-download-progress",
                    VlcDownloadProgress {
                        progress: 100.0,
                        message: "VLC 引擎下载完成".to_string(),
                    },
                );
            }
            Err(e) => {
                let _ = app_clone.emit(
                    "vlc-download-progress",
                    VlcDownloadProgress {
                        progress: -1.0,
                        message: format!("下载失败: {}", e),
                    },
                );
            }
        }
    });

    Ok(())
}

/// 检查 VLC 转码是否正在运行
#[tauri::command]
async fn vlc_stream_running(
    state: tauri::State<'_, VlcStreamState>,
) -> Result<bool, String> {
    let streamer = state.inner().0.lock().unwrap_or_else(|p| p.into_inner());
    Ok(streamer.is_running())
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
    let client = cdn_client();

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

            let sync_client = cdn_blocking_client();

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

            let sync_client = cdn_blocking_client();
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
    let client = cdn_client();

    if url.contains(".m3u8") {
        let res = client.get(url).send().await.map_err(|e| e.to_string())?;
        let text = res.text().await.map_err(|e| e.to_string())?;
        let base_url = Url::parse(url).map_err(|e| e.to_string())?;

        let mut used_names: HashMap<String, usize> = HashMap::new();
        let mut segments: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let ts_url = base_url.join(t).map_err(|e| e.to_string())?;
            let raw_name = ts_url
                .path()
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("segment.ts")
                .to_string();
            let counter = used_names.entry(raw_name.clone()).or_insert(0);
            *counter += 1;
            let name = if *counter == 1 {
                raw_name
            } else {
                format!("{}_{}", raw_name, counter)
            };
            segments.push((name, ts_url.to_string()));
        }

        let total_parts = segments.len();
        if total_parts == 0 {
            return Err("m3u8 中没有可用的分片".to_string());
        }

        let app_handle = app_handle.clone();
        let save_path = save_path.to_string();
        let live_id = live_id.to_string();
        let cancelled = cancelled.clone();

        // 分片下载 + 合并是同步文件操作，放到 blocking 线程里做，重试可以直接 sleep
        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let cache_dir = std::path::PathBuf::from(format!("{}.cache", save_path));
            std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

            let sync_client = cdn_blocking_client();

            let emit_progress = |done: usize, merging: bool| {
                let progress = if merging {
                    100.0
                } else {
                    (done as f64 / total_parts as f64) * 100.0
                };
                let _ = app_handle.emit(
                    "download-status",
                    DownloadEvent {
                        status: "progress".to_string(),
                        message: if merging {
                            format!("{} (合并分片中)", save_path)
                        } else {
                            save_path.clone()
                        },
                        progress,
                        live_id: live_id.clone(),
                    },
                );
            };

            let mut done = 0usize;
            for (name, ts_url) in &segments {
                if cancelled.load(Ordering::SeqCst) {
                    // 保留 .cache，"重试下载"可以从头续传
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

                let local = cache_dir.join(name);
                let part = cache_dir.join(format!("{}.part", name));

                // 已经下好的分片直接跳过，这就是断点续传；0 字节的坏文件删掉重下
                match std::fs::metadata(&local) {
                    Ok(meta) if meta.len() > 0 => {
                        done += 1;
                        emit_progress(done, false);
                        continue;
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&local);
                    }
                    Err(_) => {}
                }
                let _ = std::fs::remove_file(&part);

                let mut last_err = String::new();
                for attempt in 0..3u64 {
                    if attempt > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(1000 << attempt));
                    }
                    match sync_client.get(ts_url).send() {
                        Ok(mut resp) => {
                            match (|| -> Result<(), String> {
                                let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
                                std::io::copy(&mut resp, &mut file).map_err(|e| e.to_string())?;
                                file.flush().map_err(|e| e.to_string())?;
                                Ok(())
                            })() {
                                Ok(_) => {
                                    last_err.clear();
                                    break;
                                }
                                Err(e) => last_err = e,
                            }
                        }
                        Err(e) => last_err = e.to_string(),
                    }
                }

                if cancelled.load(Ordering::SeqCst) {
                    let _ = std::fs::remove_file(&part);
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

                if !local_exists(&part) {
                    let _ = std::fs::remove_file(&part);
                    return Err(format!("分片 {} 下载失败: {}", name, last_err));
                }
                std::fs::rename(&part, &local).map_err(|e| e.to_string())?;

                done += 1;
                emit_progress(done, false);
            }

            // 全部分片就位后才合并成一个 .ts：MPEG-TS 按字节顺序拼接即可
            emit_progress(done, true);
            let mut missing = 0usize;
            for (name, _) in &segments {
                if !local_exists(&cache_dir.join(name)) {
                    missing += 1;
                }
            }
            if missing > 0 {
                return Err(format!("有 {} 个分片缺失，点击重试继续下载", missing));
            }

            let out_file = std::fs::File::create(&save_path).map_err(|e| e.to_string())?;
            let mut writer = std::io::BufWriter::with_capacity(1_048_576, out_file);
            for (name, _) in &segments {
                let mut seg = std::fs::File::open(cache_dir.join(name)).map_err(|e| e.to_string())?;
                std::io::copy(&mut seg, &mut writer).map_err(|e| e.to_string())?;
            }
            writer.flush().map_err(|e| e.to_string())?;
            drop(writer);

            let _ = std::fs::remove_dir_all(&cache_dir);
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())??;
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

fn local_exists(path: &std::path::Path) -> bool {
    std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

// ===== 直播录制（独立 libVLC 会话，只落盘不上屏） =====

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveRecordStatus {
    live_id: String,
    status: String,
    file_path: String,
    elapsed: f64,
    message: String,
}

struct RecordSession {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<LiveRecordStatus>>,
}

type LiveRecordMap = Arc<Mutex<HashMap<String, RecordSession>>>;

const MAX_RECORD_SESSIONS: usize = 5;

/// 断流续录后的分段文件名：name(1).ts、name(2).ts
fn retry_output_path(save_path: &str, index: usize) -> String {
    let path = std::path::Path::new(save_path);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "record".to_string());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_else(|| ".ts".to_string());
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    dir.join(format!("{}({}){}", stem, index, ext))
        .to_string_lossy()
        .to_string()
}

fn blocking_get_play_stream_path(live_id: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();
    let res = client
        .post("https://pocketapi.48.cn/live/api/v1/live/getLiveOne")
        .headers(get_headers())
        .json(&serde_json::json!({ "liveId": live_id }))
        .send()
        .map_err(|e| e.to_string())?;
    let json: Value = res.json().map_err(|e| e.to_string())?;
    Ok(json["content"]["playStreamPath"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// 录制线程：持有自己的 FileRecorder，断流后按 48tools 的策略等 60 秒重新解析流地址续录
fn run_record_thread(
    app_handle: tauri::AppHandle,
    map: LiveRecordMap,
    live_id: String,
    url: String,
    save_path: String,
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<LiveRecordStatus>>,
) {
    let publish = |status_str: &str, file_path: &str, message: &str| {
        if let Ok(mut s) = shared.lock() {
            s.status = status_str.to_string();
            s.file_path = file_path.to_string();
            s.message = message.to_string();
        }
        if let Ok(s) = shared.lock() {
            let _ = app_handle.emit("live-record-status", s.clone());
        }
    };

    let mut recorder = match FileRecorder::start(&url, &save_path) {
        Ok(recorder) => recorder,
        Err(e) => {
            publish("error", &save_path, &e);
            if let Ok(mut sessions) = map.lock() {
                sessions.remove(&live_id);
            }
            return;
        }
    };

    let mut segment_start = std::time::Instant::now();
    let mut current_url = url.clone();
    let mut current_path = save_path.clone();
    let mut retry_index = 0usize;
    let mut final_status = "stopped";
    let mut final_message = String::new();
    // libvlc 返回 0 之后还要缓冲才进入 Playing，这段时间不能当成断流
    let mut ever_playing = false;
    let mut recorded = std::time::Duration::ZERO;

    publish("recording", &current_path, "");

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        if !recorder.is_playing() {
            if !ever_playing {
                if segment_start.elapsed().as_secs() < 30 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }
                final_status = "error";
                final_message = "VLC 未能开始录制".to_string();
                break;
            }

            // player_is_playing 归零：流断了（主播下播或掉线）
            publish("retrying", &current_path, "直播中断，60 秒后尝试续录");
            let mut stopped_by_user = false;
            for _ in 0..60 {
                if stop.load(Ordering::SeqCst) {
                    stopped_by_user = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            if stopped_by_user {
                break;
            }

            match blocking_get_play_stream_path(&live_id) {
                Ok(new_url) if !new_url.is_empty() && new_url != current_url => {
                    retry_index += 1;
                    let new_path = retry_output_path(&save_path, retry_index);
                    match recorder.restart(&new_url, &new_path) {
                        Ok(()) => {
                            current_url = new_url;
                            current_path = new_path.clone();
                            ever_playing = false;
                            segment_start = std::time::Instant::now();
                            publish(
                                "recording",
                                &new_path,
                                &format!("主播重连，续录到第 {} 个文件", retry_index),
                            );
                        }
                        Err(e) => {
                            final_status = "error";
                            final_message = e;
                            break;
                        }
                    }
                }
                Ok(_) => break,
                Err(e) => {
                    final_status = "error";
                    final_message = e;
                    break;
                }
            }
            continue;
        }

        ever_playing = true;
        recorded += std::time::Duration::from_secs(1);
        if let Ok(mut s) = shared.lock() {
            s.elapsed = recorded.as_secs_f64();
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    if stop.load(Ordering::SeqCst) {
        final_status = "stopped";
        final_message = "已手动停止录制".to_string();
    }
    publish(final_status, &current_path, &final_message);

    drop(recorder);
    if let Ok(mut sessions) = map.lock() {
        sessions.remove(&live_id);
    }
}

#[tauri::command]
async fn start_live_record(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, LiveRecordMap>,
    live_id: String,
    url: String,
    save_path: String,
) -> Result<LiveRecordStatus, String> {
    let map = state.inner().clone();
    let (stop, shared) = {
        let mut sessions = map.lock().map_err(|e| e.to_string())?;
        if sessions.contains_key(&live_id) {
            return Err("该直播已经在录制中".to_string());
        }
        if sessions.len() >= MAX_RECORD_SESSIONS {
            return Err(format!("同时录制数量已达上限（{}）", MAX_RECORD_SESSIONS));
        }
        let shared = Arc::new(Mutex::new(LiveRecordStatus {
            live_id: live_id.clone(),
            status: "recording".to_string(),
            file_path: save_path.clone(),
            elapsed: 0.0,
            message: String::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        sessions.insert(
            live_id.clone(),
            RecordSession {
                stop: stop.clone(),
                status: shared.clone(),
            },
        );
        (stop, shared)
    };

    let initial = shared.lock().map_err(|e| e.to_string())?.clone();
    std::thread::spawn(move || {
        run_record_thread(app_handle, map, live_id, url, save_path, stop, shared)
    });

    Ok(initial)
}

#[tauri::command]
async fn stop_live_record(live_id: String, state: tauri::State<'_, LiveRecordMap>) -> Result<(), String> {
    let sessions = state.inner().lock().map_err(|e| e.to_string())?;
    match sessions.get(&live_id) {
        Some(session) => {
            session.stop.store(true, Ordering::SeqCst);
            Ok(())
        }
        None => Err("该直播没有在录制".to_string()),
    }
}

#[tauri::command]
async fn get_live_record_list(
    state: tauri::State<'_, LiveRecordMap>,
) -> Result<Vec<LiveRecordStatus>, String> {
    let sessions = state.inner().lock().map_err(|e| e.to_string())?;
    let mut list = Vec::with_capacity(sessions.len());
    for session in sessions.values() {
        if let Ok(status) = session.status.lock() {
            list.push(status.clone());
        }
    }
    Ok(list)
}

/// 自动抓取日志：往保存目录追加 pocket48_live_log.txt
#[tauri::command]
fn append_record_log(dir: String, content: String) -> Result<(), String> {
    let path = std::path::Path::new(&dir).join("pocket48_live_log.txt");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("写入 {} 失败: {}", path.display(), e))?;
    writeln!(file, "{}", content).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())
}

// ===== Mini-Player Window Commands =====

#[derive(Clone, serde::Serialize)]
struct MiniPlayerConfig {
    title: String,
    nickname: String,
    stream_url: String,
    is_live: bool,
}

#[derive(Default)]
struct MiniPlayerState(Mutex<Option<MiniPlayerConfig>>);

#[tauri::command]
async fn create_mini_player(
    app: tauri::AppHandle,
    state: tauri::State<'_, MiniPlayerState>,
    title: String,
    nickname: String,
    stream_url: String,
    is_live: bool,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
) -> Result<(), String> {
    // Published before the window exists so the page always finds it when it asks.
    *state.0.lock().map_err(|e| e.to_string())? = Some(MiniPlayerConfig {
        title,
        nickname,
        stream_url,
        is_live,
    });

    // `close()` only requests closure and leaves the "mini-player" label owned by the
    // old window while `build()` below runs, which fails with WebviewLabelAlreadyExists.
    if let Some(existing) = app.get_webview_window("mini-player") {
        let _ = existing.destroy();
    }

    let base_url = if cfg!(debug_assertions) {
        "http://localhost:1420?mini=true".to_string()
    } else {
        "index.html?mini=true".to_string()
    };

    let (pos_x, pos_y) = if x < 0.0 || y < 0.0 {
        if let Some(monitor) = app.primary_monitor().ok().flatten() {
            let area = monitor.work_area();
            let scale = monitor.scale_factor();
            let margin = 20.0;
            (
                (area.size.width as f64 / scale) - width - margin,
                (area.size.height as f64 / scale) - height - margin,
            )
        } else {
            (100.0, 100.0)
        }
    } else {
        (x, y)
    };

    WebviewWindowBuilder::new(
        &app,
        "mini-player",
        tauri::WebviewUrl::App(base_url.into()),
    )
    .title("live48 - 小窗播放")
    .inner_size(width, height)
    .position(pos_x, pos_y)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(true)
    // 原生 resize 时窗口刚扩出来的那条边还没被 webview 重绘，默认底色会闪出一条
    // 白边（看起来像边缘重影），先把窗口和 webview 的底色都压成黑色。
    .background_color(tauri::window::Color(0, 0, 0, 255))
    .focused(true)
    .build()
    .map_err(|e| format!("创建小窗失败: {}", e))?;

    Ok(())
}

#[tauri::command]
fn get_mini_player_config(state: tauri::State<'_, MiniPlayerState>) -> Option<MiniPlayerConfig> {
    state.0.lock().ok().and_then(|config| config.clone())
}

#[tauri::command]
async fn close_mini_player(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("mini-player") {
        window.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn hide_main_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebView2 requires a user gesture in the frame before it lets a <video> play.
    // The mini-player window is created from a click that happened in the *main* window,
    // so its own frame has no gesture and playback would be rejected. Must be set before
    // the first webview boots the shared browser process.
    #[cfg(windows)]
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--autoplay-policy=no-user-gesture-required",
    );

    // 启动阶段确保 VLC DLL 就绪（如缺失会自动下载，避免后续在持有锁时下载）
    vlc_player::ensure_vlc_downloaded();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .manage::<CancelMap>(Arc::new(Mutex::new(HashMap::new())))
        .manage::<LiveRecordMap>(Arc::new(Mutex::new(HashMap::new())))
        .manage(VlcStreamState(Mutex::new(VlcStreamer::new())))
        .manage(MiniPlayerState::default())
        .invoke_handler(tauri::generate_handler![
            get_live_list,
            get_record_list,
            get_user_record_list,
            get_open_live_list,
            get_open_live_streams,
            get_record_danmu,
            download_danmu_file,
            get_live_stream_url,
            get_live_stream_url_v2,
            get_playable_stream_url,
            open_in_player,
            open_in_vlc,
            vlc_play,
            vlc_stop,
            check_vlc_installed,
            check_vlc_integrity,
            download_vlc,
            vlc_stream_running,
            download_video,
            cancel_download,
            start_live_record,
            stop_live_record,
            get_live_record_list,
            append_record_log,
            create_mini_player,
            get_mini_player_config,
            close_mini_player,
            hide_main_window,
            show_main_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}