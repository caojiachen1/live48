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
            // A bare number never appears for lives worth re-resolving; liveIdStr wins
            // anyway when the server sends both.
            IdField::Num(_) => "",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Source {
    /// A member's own live/record room (`getLiveList` + `getLiveOne`).
    #[default]
    Member,
    /// An official theatre performance (`getOpenLiveList` + `getOpenLiveOne`).
    Open,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct UserInfo {
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(rename = "userId", default)]
    pub user_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct LiveItem {
    pub live_id: String,
    pub title: String,
    pub sub_title: String,
    pub cover_path: String,
    pub live_type: i64,
    /// 0 = a real live, 1 = the member is only pushing a recording.
    pub live_mode: i64,
    /// Open lives only: 1 not started, 2 on air, 4 finished.
    pub status: i64,
    pub in_microphone_connection: bool,
    pub ctime: Option<i64>,
    /// Records advertise their length as `mm:ss` (or `h:mm:ss`).
    pub duration: String,
    pub nickname: String,
    pub avatar: String,
    pub user_id: String,
    pub room_id: String,
    pub source: Source,
}

#[derive(Deserialize)]
struct RawLiveItem {
    #[serde(rename = "liveId", default)]
    live_id: IdField,
    #[serde(rename = "liveIdStr", default)]
    live_id_str: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "subTitle", default)]
    sub_title: Option<String>,
    #[serde(rename = "coverPath", default)]
    cover_path: Option<String>,
    #[serde(rename = "liveType", default)]
    live_type: i64,
    #[serde(rename = "liveMode", default)]
    live_mode: i64,
    #[serde(default)]
    status: i64,
    #[serde(default)]
    duration: Option<String>,
    #[serde(rename = "inMicrophoneConnection", default = "default_false")]
    in_microphone_connection: bool,
    #[serde(default)]
    ctime: Option<String>,
    #[serde(rename = "stime", default)]
    stime: Option<String>,
    #[serde(rename = "roomId", default)]
    room_id: Option<String>,
    #[serde(rename = "userInfo", default)]
    user_info: UserInfo,
    #[serde(rename = "teamList", default)]
    team_list: Vec<RawTeam>,
}

#[derive(Deserialize)]
struct RawTeam {
    #[serde(rename = "teamName", default)]
    team_name: String,
}

impl From<RawLiveItem> for LiveItem {
    fn from(r: RawLiveItem) -> Self {
        let team = r
            .team_list
            .first()
            .map(|t| t.team_name.clone())
            .unwrap_or_default();
        let sub_title = r
            .sub_title
            .filter(|s| !s.is_empty())
            .or(if team.is_empty() { None } else { Some(team) })
            .unwrap_or_default();
        LiveItem {
            live_id: r.live_id_str.clone().unwrap_or_else(|| r.live_id.as_str().to_owned()),
            title: r.title.unwrap_or_default(),
            sub_title,
            cover_path: r.cover_path.unwrap_or_default(),
            live_type: r.live_type,
            live_mode: r.live_mode,
            status: r.status,
            in_microphone_connection: r.in_microphone_connection,
            ctime: r
                .ctime
                .as_deref()
                .or(r.stime.as_deref())
                .and_then(|s| s.parse().ok()),
            duration: r.duration.unwrap_or_default(),
            nickname: r.user_info.nickname,
            avatar: r.user_info.avatar,
            user_id: r.user_info.user_id,
            room_id: r.room_id.unwrap_or_default(),
            source: Source::Member,
        }
    }
}

#[derive(Deserialize)]
struct ListContent {
    #[serde(rename = "liveList", default)]
    live_list: Vec<RawLiveItem>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    content: Option<ListContent>,
}

#[derive(Deserialize)]
struct RawStream {
    #[serde(rename = "streamName", default)]
    stream_name: String,
    #[serde(rename = "streamType", default)]
    stream_type: i64,
    #[serde(rename = "streamPath", default)]
    stream_path: String,
    #[serde(rename = "vipShow", default)]
    vip: bool,
}

#[derive(Clone, Debug)]
pub struct Stream {
    pub name: String,
    pub stream_type: i64,
    pub path: String,
    pub vip: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RoomInfo {
    pub room_id: String,
    pub play_stream_path: Option<String>,
    pub streams: Vec<Stream>,
    /// A `.lrc` file with the record's danmaku; empty for lives and theatre shows.
    pub msg_file_path: Option<String>,
}

#[derive(Deserialize)]
struct RoomInfoContent {
    #[serde(rename = "roomId", default)]
    room_id: String,
    #[serde(rename = "playStreamPath", default)]
    play_stream_path: Option<String>,
    #[serde(rename = "playStreams", default)]
    play_streams: Vec<RawStream>,
    #[serde(rename = "msgFilePath", default)]
    msg_file_path: Option<String>,
}

#[derive(Deserialize)]
struct RoomInfoResponse {
    #[serde(default)]
    content: Option<RoomInfoContent>,
}

/// liveType -> badge label, matching the Pocket48 app's own tags. A pushed recording
/// outranks the type, which is what the original client does.
pub fn badge_text(item: &LiveItem) -> &'static str {
    if item.source == Source::Open {
        return match item.status {
            1 => "未开始",
            2 => "公演中",
            _ => "公演",
        };
    }
    if item.live_mode == 1 {
        "录屏"
    } else if item.live_type == 1 {
        "视频"
    } else if item.live_type == 2 {
        "电台"
    } else if item.live_type == 3 || item.in_microphone_connection {
        "连麦"
    } else if item.live_type == 5 {
        "游戏"
    } else if item.live_type == 6 {
        "AI"
    } else {
        "未知"
    }
}

pub fn parse_live_list(json: &str) -> Result<(Vec<LiveItem>, String), String> {
    let res: ListResponse =
        serde_json::from_str(json).map_err(|e| format!("列表解析失败: {e}"))?;
    match res.content {
        Some(c) => {
            let next = c.next.unwrap_or_else(|| "0".to_string());
            Ok((
                c.live_list
                    .into_iter()
                    .map(|i| {
                        let mut item: LiveItem = i.into();
                        item.source = Source::Member;
                        item
                    })
                    .collect(),
                next,
            ))
        }
        None => Err("响应中没有 content".to_string()),
    }
}

/// `getOpenLiveList` shares the list envelope but its items carry a team instead of a
/// member, and `stime` rather than `ctime`.
pub fn parse_open_live_list(json: &str) -> Result<(Vec<LiveItem>, String), String> {
    let res: ListResponse =
        serde_json::from_str(json).map_err(|e| format!("公演列表解析失败: {e}"))?;
    match res.content {
        Some(c) => {
            let next = c.next.unwrap_or_else(|| "0".to_string());
            let items = c
                .live_list
                .into_iter()
                .map(|i| {
                    let mut item: LiveItem = i.into();
                    item.source = Source::Open;
                    item
                })
                .collect();
            Ok((items, next))
        }
        None => Err("响应中没有 content".to_string()),
    }
}

pub fn parse_room_info(json: &str) -> Result<RoomInfo, String> {
    let res: RoomInfoResponse =
        serde_json::from_str(json).map_err(|e| format!("房间信息解析失败: {e}"))?;
    let c = res.content.ok_or_else(|| "响应中没有 content".to_string())?;
    let is_rtmp = |s: &str| s.starts_with("rtmp://") || s.starts_with("rtmps://");
    let streams: Vec<Stream> = c
        .play_streams
        .into_iter()
        .filter(|s| !s.stream_path.is_empty() && !is_rtmp(&s.stream_path))
        .map(|s| Stream {
            name: s.stream_name,
            stream_type: s.stream_type,
            path: s.stream_path,
            vip: s.vip,
        })
        .collect();
    Ok(RoomInfo {
        room_id: c.room_id,
        play_stream_path: c.play_stream_path,
        streams,
        msg_file_path: c.msg_file_path.filter(|s| !s.is_empty()),
    })
}

/// An explicit `streamType` wins; otherwise prefer the best non-RTMP address and only
/// fall back to RTMP, which the built-in libVLC player can open but a system player
/// usually cannot.
pub fn pick_stream(info: &RoomInfo, want: Option<i64>) -> Result<String, String> {
    if let Some(t) = want {
        if let Some(s) = info.streams.iter().find(|s| s.stream_type == t) {
            return Ok(s.path.clone());
        }
    }
    for st in [3i64, 1, 2] {
        if let Some(s) = info.streams.iter().find(|s| s.stream_type == st) {
            return Ok(s.path.clone());
        }
    }
    if let Some(s) = info.streams.first() {
        return Ok(s.path.clone());
    }
    if let Some(p) = &info.play_stream_path {
        if !p.starts_with("rtmp") {
            return Ok(p.clone());
        }
    }
    info.play_stream_path
        .clone()
        .ok_or_else(|| "未能获取到可用的流地址".to_string())
}

#[derive(Clone, Debug)]
pub struct Danmu {
    pub time: String,
    pub secs: f64,
    pub nickname: String,
    pub message: String,
}

/// A record's `.lrc` is one danmaku per line: `[00:05:43.453]昵称\t内容`, and the
/// millisecond field is not always three digits wide.
pub fn parse_danmaku(text: &str) -> Vec<Danmu> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if !line.starts_with('[') {
            continue;
        }
        let Some(close) = line.find(']') else { continue };
        let stamp = &line[1..close];
        let Some(secs) = stamp_to_secs(stamp) else { continue };
        let rest = &line[close + 1..];
        let (nickname, message) = match rest.split_once('\t') {
            Some((n, m)) => (n.trim().to_owned(), m.to_owned()),
            None => (String::new(), rest.trim().to_owned()),
        };
        out.push(Danmu {
            time: stamp.to_owned(),
            secs,
            nickname,
            message,
        });
    }
    out
}

fn stamp_to_secs(stamp: &str) -> Option<f64> {
    let mut it = stamp.split(':');
    let h: f64 = it.next()?.parse().ok()?;
    let m: f64 = it.next()?.parse().ok()?;
    let s: f64 = it.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}
