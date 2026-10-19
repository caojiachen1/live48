use crate::models::{self, LiveItem};
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_LANGUAGE, CONTENT_TYPE, HOST, USER_AGENT};

const BASE: &str = "https://pocketapi.48.cn/live/api/v1/live";

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
        let json = self.text(
            "getLiveList",
            &serde_json::json!({ "debug": true, "next": next, "groupId": 0, "record": true }),
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

    fn get_live_one(&self, live_id: &str) -> Result<String, String> {
        self.text("getLiveOne", &serde_json::json!({ "liveId": live_id }))
    }

    fn get_live_stream_v2(&self, live_id: &str) -> Result<String, String> {
        self.text(
            "getlivestream",
            &serde_json::json!({ "liveId": live_id, "streamType": 3 }),
        )
    }

    /// Prefer a non-RTMP address; falls back to RTMP, which the built-in libVLC
    /// player can open directly but an external system player may not.
    pub fn playable_stream_url(&self, live_id: &str) -> Result<String, String> {
        let one = self.get_live_one(live_id)?;
        let v2 = self.get_live_stream_v2(live_id).ok();
        models::parse_stream_urls(&one, v2.as_deref())
    }

    /// The original `get_live_stream_url`: raw `playStreamPath` from getLiveOne.
    pub fn stream_url(&self, live_id: &str) -> Result<String, String> {
        let one = self.get_live_one(live_id)?;
        models::parse_plain_stream(&one)
            .ok_or_else(|| "playStreamPath 解析失败".to_string())
    }

    pub fn download_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| e.to_string())?;
        res.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
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
        let res = self
            .client
            .get(url)
            .send()
            .map_err(|e| e.to_string())?;
        res.text().map_err(|e| e.to_string())
    }

    pub fn content_length(&self, url: &str) -> u64 {
        match self.client.head(url).send() {
            Ok(r) => r.content_length().unwrap_or(0),
            Err(_) => 0,
        }
    }

    pub fn get(&self, url: &str) -> Result<Response, String> {
        self.client.get(url).send().map_err(|e| e.to_string())
    }
}

pub fn resolve_image(path: &str) -> String {
    if path.starts_with("http") {
        path.to_owned()
    } else {
        format!("https://source3.48.cn{path}")
    }
}
