use reqwest::blocking::{Client, Response};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT_LANGUAGE, CONTENT_TYPE, HOST, USER_AGENT,
};

use crate::models::{self, Danmu, LiveItem, RoomInfo, Source};

const BASE: &str = "https://pocketapi.48.cn/live/api/v1/live";
/// The CDN in front of the record playlists answers `403` for a generic client on some
/// hosts, and the app's own engine identity is what it always accepts.
const ENGINE_UA: &str = "SNH48 ENGINE";

/// `getLiveList` takes a group id, not a group name.
pub const GROUP_IDS: [(&str, i64); 11] = [
    ("SNH48", 10),
    ("BEJ48", 11),
    ("GNZ48", 12),
    ("CKG48", 14),
    ("CGT48", 21),
    ("IDFT", 15),
    ("海外练习生", 16),
    ("THE9", 17),
    ("硬糖少女303", 18),
    ("明星殿堂", 19),
    ("丝芭影视", 20),
];

pub fn group_id(name: &str) -> Option<i64> {
    GROUP_IDS.iter().find(|(n, _)| *n == name).map(|(_, id)| *id)
}

fn headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(HOST, HeaderValue::from_static("pocketapi.48.cn"));
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json;charset=utf-8"),
    );
    h.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "PocketFans201807/6.0.16 (iPhone; iOS 13.5.1; Scale/2.00)",
        ),
    );
    h.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-Hans-AW;q=1"));
    h.insert(
        "appInfo",
        HeaderValue::from_static(
            r#"{"vendor":"apple","deviceId":"11111111-1111-1111-1111-111111111111","appVersion":"7.0.4","appBuild":"23011601","osVersion":"16.3.1","osType":"ios","deviceName":"iPhone XR","os":"ios"}"#,
        ),
    );
    h
}

pub struct Api {
    client: Client,
}

impl Api {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("reqwest client");
        Api { client }
    }

    fn post(&self, path: &str, body: &serde_json::Value) -> Result<Response, String> {
        self.client
            .post(format!("{BASE}/{path}"))
            .headers(headers())
            .json(body)
            .send()
            .map_err(|e| e.to_string())
    }

    fn text(&self, path: &str, body: &serde_json::Value) -> Result<String, String> {
        let res = self.post(path, body)?;
        res.text().map_err(|e| e.to_string())
    }

    pub fn live_list(&self) -> Result<Vec<LiveItem>, String> {
        let json = self.text(
            "getLiveList",
            &serde_json::json!({ "debug": true, "next": "0", "groupId": 0, "record": false }),
        )?;
        models::parse_live_list(&json).map(|(items, _)| items)
    }

    pub fn record_page(&self, next: &str) -> Result<(Vec<LiveItem>, String), String> {
        self.record_page_group(next, 0)
    }

    pub fn record_page_group(
        &self,
        next: &str,
        group_id: i64,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let json = self.text(
            "getLiveList",
            &serde_json::json!({ "debug": true, "next": next, "groupId": group_id, "record": true }),
        )?;
        models::parse_live_list(&json)
    }

    /// One page of a single member's records. `next` must not be `"0"` here: the server
    /// silently returns nothing for `userId` + `next=0`, so the caller has to hand it a
    /// real cursor - which is why [records_for_user] seeds itself from the newest record.
    pub fn record_page_user(
        &self,
        next: &str,
        user_id: &str,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let json = self.text(
            "getLiveList",
            &serde_json::json!({
                "debug": true,
                "next": next,
                "groupId": 0,
                "record": true,
                "userId": user_id,
            }),
        )?;
        models::parse_live_list(&json)
    }

    /// Walk pages back until the newest-oldest cursor passes `target_ms`
    /// (the original loads yesterday-00:00 on first run, then one more day per "load more").
    pub fn records_back_to(
        &self,
        from_next: &str,
        target_ms: i64,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let mut next = from_next.to_string();
        let mut acc: Vec<LiveItem> = Vec::new();
        loop {
            if next == "0" && !acc.is_empty() {
                break;
            }
            let (items, cursor) = self.record_page(&next)?;
            let reached = items.last().and_then(|i| i.ctime).unwrap_or(i64::MAX) < target_ms;
            acc.extend(items);
            next = cursor;
            if reached || next == "0" {
                break;
            }
        }
        Ok((acc, next))
    }

    pub fn records_for_user(
        &self,
        user_id: &str,
        target_ms: i64,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let (seed, cursor) = self.record_page("0")?;
        let newest_is_target = seed
            .first()
            .is_some_and(|i| i.user_id == user_id || i.nickname == user_id);
        let seed_first = seed.first().cloned();
        let mut acc: Vec<LiveItem> = Vec::new();
        if newest_is_target {
            if let Some(first) = seed_first.clone() {
                acc.push(first);
            }
        }
        let mut next = if cursor == "0" {
            seed_first.map(|i| i.live_id).unwrap_or_default()
        } else {
            cursor
        };
        if next.is_empty() {
            return Ok((acc, "0".to_owned()));
        }
        loop {
            let (items, cursor) = self.record_page_user(&next, user_id)?;
            let reached = items.last().and_then(|i| i.ctime).unwrap_or(i64::MAX) < target_ms;
            acc.extend(items);
            next = cursor;
            if reached || next == "0" || next.is_empty() {
                break;
            }
        }
        Ok((acc, next))
    }

    /// `getOpenLiveList`: theatre shows. `record=false` is "on air or scheduled",
    /// `record=true` the archive, and the cursor is a millisecond stamp.
    pub fn open_live_list(
        &self,
        record: bool,
        next: &str,
        group: i64,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let json = self.text(
            "getOpenLiveList",
            &serde_json::json!({
                "debug": false,
                "groupId": group,
                "next": next.parse::<i64>().unwrap_or(0),
                "record": record,
            }),
        )?;
        models::parse_open_live_list(&json)
    }

    fn path_for(source: Source) -> &'static str {
        match source {
            Source::Member => "getLiveOne",
            Source::Open => "getOpenLiveOne",
        }
    }

    pub fn room_info(&self, live_id: &str, source: Source) -> Result<RoomInfo, String> {
        let json = self.text(Self::path_for(source), &serde_json::json!({ "liveId": live_id }))?;
        models::parse_room_info(&json)
    }

    fn get_live_stream_v2(&self, live_id: &str) -> Result<String, String> {
        self.text(
            "getlivestream",
            &serde_json::json!({ "liveId": live_id, "streamType": 3 }),
        )
    }

    /// Prefer a non-RTMP address; falls back to RTMP, which the built-in libVLC
    /// player can open directly but an external system player may not.
    pub fn playable_stream_url(
        &self,
        live_id: &str,
        source: Source,
        want_type: Option<i64>,
    ) -> Result<String, String> {
        let info = self.room_info(live_id, source)?;
        if let Some(path) = models::pick_stream(&info, want_type).ok() {
            return Ok(path);
        }
        if source == Source::Member {
            if let Ok(v2) = self.get_live_stream_v2(live_id) {
                if let Some(serde_json::Value::String(p)) =
                    serde_json::from_str::<serde_json::Value>(&v2)
                        .ok()
                        .and_then(|v| v.get("content").cloned())
                {
                    if !p.is_empty() && !p.starts_with("rtmp") {
                        return Ok(p);
                    }
                }
            }
        }
        models::pick_stream(&info, None)
            .or_else(|_| info.play_stream_path.clone().ok_or("未能获取到可用的流地址".to_string()))
    }

    /// The raw `playStreamPath` / best stream, which is what libVLC wants: it demuxes
    /// RTMP itself, so there is no reason to prefer the weaker HTTPS copy.
    pub fn stream_url(&self, live_id: &str, source: Source, want_type: Option<i64>) -> Result<String, String> {
        let info = self.room_info(live_id, source)?;
        if let Some(t) = want_type {
            if let Some(s) = info.streams.iter().find(|s| s.stream_type == t) {
                return Ok(s.path.clone());
            }
        }
        match info.play_stream_path.clone() {
            Some(p) if !p.is_empty() => Ok(p),
            _ => models::pick_stream(&info, None),
        }
    }

    /// A record's danmaku sits in a plain `.lrc` next to the stream address.
    pub fn danmaku(&self, url: &str) -> Result<Vec<Danmu>, String> {
        let text = self.engine_get(url)?.text().map_err(|e| e.to_string())?;
        Ok(models::parse_danmaku(&text))
    }

    fn engine_get(&self, url: &str) -> Result<Response, String> {
        self.client
            .get(url)
            .header(USER_AGENT, HeaderValue::from_static(ENGINE_UA))
            .send()
            .map_err(|e| e.to_string())
    }

    /// Covers and avatars come in dozens per screen, and building a `Client` costs a
    /// fresh TLS context and connection pool, so they share one.
    pub fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
        static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
        let client = CLIENT.get_or_init(|| {
            Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .pool_max_idle_per_host(16)
                .build()
                .expect("image client")
        });
        let res = client.get(url).send().map_err(|e| e.to_string())?;
        res.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
    }

    pub fn download_text(&self, url: &str) -> Result<String, String> {
        let res = self.engine_get(url)?;
        res.text().map_err(|e| e.to_string())
    }

    pub fn content_length(&self, url: &str) -> u64 {
        match self.client.get(url).header(USER_AGENT, HeaderValue::from_static(ENGINE_UA)).send() {
            Ok(r) => r.content_length().unwrap_or(0),
            Err(_) => 0,
        }
    }

    pub fn get(&self, url: &str) -> Result<Response, String> {
        self.engine_get(url)
    }
}

pub fn resolve_image(path: &str) -> String {
    if path.starts_with("http") {
        path.to_owned()
    } else {
        format!("https://source3.48.cn{path}")
    }
}
