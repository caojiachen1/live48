use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE, CONTENT_TYPE, HOST, USER_AGENT};
use serde_json::Value;
use std::process::Command;

// 构造统一的请求头
fn get_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("pocketapi.48.cn"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json;charset=utf-8"));
    headers.insert(USER_AGENT, HeaderValue::from_static("PocketFans201807/6.0.16 (iPhone; iOS 13.5.1; Scale/2.00)"));
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

    let res = client.post("https://pocketapi.48.cn/live/api/v1/live/getLiveList")
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

    let res = client.post("https://pocketapi.48.cn/live/api/v1/live/getLiveOne")
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
                .args(&["-NoProfile", "-WindowStyle", "Hidden", "-Command", &format!("Start-Process '{}'", url.replace("'", "''"))])
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_live_list, get_live_stream_url, open_in_player])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
