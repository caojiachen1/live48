use serde::Deserialize;

fn default_false() -> bool {
    false
}

/// `liveId` is a string in the API but tolerate a bare number.
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
enum IdField {
    Str(String),
    Num(i64),
}

impl Default for IdField {
    fn default() -> Self {
        IdField::Str(String::new())
    }
}

impl IdField {
    fn as_str(&self) -> &str {
        match self {
            IdField::Str(s) => s,
            IdField::Num(_) => "",
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct UserInfo {
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub avatar: String,
}

#[derive(Clone, Debug, Default)]
pub struct LiveItem {
    pub live_id: String,
    pub title: String,
    pub cover_path: String,
    pub live_type: i64,
    pub in_microphone_connection: bool,
    pub ctime: Option<i64>,
    pub nickname: String,
    pub avatar: String,
}

#[derive(Deserialize)]
struct RawLiveItem {
    #[serde(rename = "liveId", default)]
    live_id: IdField,
    #[serde(rename = "liveIdStr", default)]
    live_id_str: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "coverPath", default)]
    cover_path: Option<String>,
    #[serde(rename = "liveType", default)]
    live_type: i64,
    #[serde(rename = "inMicrophoneConnection", default = "default_false")]
    in_microphone_connection: bool,
    #[serde(default)]
    ctime: Option<String>,
    #[serde(rename = "userInfo", default)]
    user_info: UserInfo,
}

impl From<RawLiveItem> for LiveItem {
    fn from(r: RawLiveItem) -> Self {
        let live_id = r
            .live_id_str
            .clone()
            .unwrap_or_else(|| r.live_id.as_str().to_owned());
        LiveItem {
            live_id,
            title: r.title.unwrap_or_default(),
            cover_path: r.cover_path.unwrap_or_default(),
            live_type: r.live_type,
            in_microphone_connection: r.in_microphone_connection,
            ctime: r.ctime.and_then(|s| s.parse().ok()),
            nickname: r.user_info.nickname,
            avatar: r.user_info.avatar,
        }
    }
}

#[derive(Deserialize)]
struct LiveListContent {
    #[serde(rename = "liveList", default)]
    live_list: Vec<RawLiveItem>,
    #[serde(rename = "next", default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct LiveListResponse {
    #[serde(default)]
    content: Option<LiveListContent>,
}

#[derive(Deserialize)]
struct StreamOneContent {
    #[serde(rename = "playStreamPath", default)]
    play_stream_path: Option<String>,
    #[serde(rename = "playStreams", default)]
    play_streams: Option<Vec<PlayStream>>,
}

#[derive(Deserialize)]
struct PlayStream {
    #[serde(rename = "streamType", default)]
    stream_type: i64,
    #[serde(rename = "streamPath", default)]
    stream_path: String,
}

#[derive(Deserialize)]
struct StreamOneResponse {
    #[serde(default)]
    content: Option<StreamOneContent>,
}

#[derive(Deserialize)]
struct StreamV2Response {
    #[serde(default)]
    content: Option<serde_json::Value>,
}

/// liveType -> badge label, matching the Pocket48 app's own tags.
pub fn badge_text(live_type: i64, linked: bool) -> &'static str {
    if live_type == 1 {
        "视频"
    } else if live_type == 2 {
        "电台"
    } else if live_type == 3 || linked {
        "连麦"
    } else if live_type == 5 {
        "游戏"
    } else if live_type == 6 {
        "AI"
    } else {
        "未知"
    }
}

pub fn parse_live_list(json: &str) -> Result<(Vec<LiveItem>, String), String> {
    let res: LiveListResponse =
        serde_json::from_str(json).map_err(|e| format!("列表解析失败: {e}"))?;
    match res.content {
        Some(c) => {
            let next = c.next.unwrap_or_else(|| "0".to_string());
            Ok((c.live_list.into_iter().map(Into::into).collect(), next))
        }
        None => Err("响应中没有 content".to_string()),
    }
}

/// Mirrors the original `getLiveOne` + `getlivestream` fallback chain: prefer an
/// HTTPS stream, and only hand back RTMP when nothing else exists.
pub fn parse_stream_urls(one_json: &str, v2_json: Option<&str>) -> Result<String, String> {
    let is_rtmp = |s: &str| s.starts_with("rtmp://") || s.starts_with("rtmps://");

    if let Ok(one) = serde_json::from_str::<StreamOneResponse>(one_json) {
        if let Some(c) = &one.content {
            if let Some(streams) = &c.play_streams {
                for st in [3i64, 1, 2] {
                    for s in streams {
                        if s.stream_type == st && !is_rtmp(&s.stream_path) {
                            return Ok(s.stream_path.clone());
                        }
                    }
                }
            }
            if let Some(p) = &c.play_stream_path {
                if !is_rtmp(p) {
                    return Ok(p.clone());
                }
            }
        }
    }

    if let Some(json) = v2_json {
        if let Ok(v2) = serde_json::from_str::<StreamV2Response>(json) {
            if let Some(serde_json::Value::String(p)) = v2.content {
                if !is_rtmp(&p) {
                    return Ok(p);
                }
            }
        }
    }

    // Nothing but RTMP is available; the built-in player can still handle it.
    if let Ok(one) = serde_json::from_str::<StreamOneResponse>(one_json) {
        if let Some(p) = one.content.and_then(|c| c.play_stream_path) {
            return Ok(p);
        }
    }

    Err("未能获取到可用的流地址".to_string())
}

pub fn parse_plain_stream(one_json: &str) -> Option<String> {
    serde_json::from_str::<StreamOneResponse>(one_json)
        .ok()
        .and_then(|r| r.content)
        .and_then(|c| c.play_stream_path)
}
