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
    // Same shape as the client itself: PocketFans201807/<version> (platform…).
    h.insert(
        USER_AGENT,
        HeaderValue::from_static("PocketFans201807/7.1.45 (iPhone; iOS 26.1; Scale/3.00)"),
    );
    h.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-Hans-AW;q=1"));
    h.insert(
        "appInfo",
        HeaderValue::from_static(
            r#"{"vendor":"apple","deviceId":"11111111-1111-1111-1111-111111111111","appVersion":"7.1.45","appBuild":"26110901","osVersion":"26.1","osType":"ios","deviceName":"iPhone 16 Pro","os":"ios"}"#,
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

    pub fn live_list(&self, group_id: i64, team_id: i64) -> Result<Vec<LiveItem>, String> {
        let json = self.text(
            "getLiveList",
            &serde_json::json!({
                "debug": false,
                "next": "0",
                "groupId": group_id,
                "teamId": team_id,
                "userId": 0,
                "record": false,
            }),
        )?;
        models::parse_live_list(&json).map(|(items, _)| items)
    }

    pub fn record_page_group(
        &self,
        next: &str,
        group_id: i64,
        team_id: i64,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let json = self.text(
            "getLiveList",
            &serde_json::json!({
                "debug": false,
                "next": next,
                "record": true,
                "groupId": group_id,
                "teamId": team_id,
                "userId": 0,
            }),
        )?;
        models::parse_live_list(&json)
    }

    /// One page of one member's live records: the same list endpoint with a `userId`.
    pub fn record_page_user(
        &self,
        next: &str,
        user_id: &str,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let json = self.text(
            "getLiveList",
            &serde_json::json!({
                "debug": false,
                "next": next,
                "record": true,
                "groupId": 0,
                "teamId": 0,
                "userId": user_id.parse::<i64>().unwrap_or(0),
            }),
        )?;
        models::parse_live_list(&json)
    }

    /// The theatre playbacks a member took part in. The app asks for this next to
    /// `getOpenLiveOne` on a show's page, which is why the rows are opened as
    /// theatre items even though the request only carries the member id.
    pub fn member_playback(&self, star_id: &str) -> Result<(Vec<LiveItem>, String), String> {
        let json = self.text(
            "getHistoryRecord",
            &serde_json::json!({ "starId": star_id.parse::<i64>().unwrap_or(0), "next": 0 }),
        )?;
        let (mut items, next) = models::parse_history(&json)?;
        for item in &mut items {
            item.source = Source::Open;
        }
        Ok((items, next))
    }

    /// Raw page of `getHistoryRecord`, kept for the self-check output.
    pub fn history_records(
        &self,
        star_id: &str,
        next: &str,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let json = self.text(
            "getHistoryRecord",
            &serde_json::json!({ "starId": star_id.parse::<i64>().unwrap_or(0), "next": next.parse::<i64>().unwrap_or(0) }),
        )?;
        models::parse_history(&json)
    }

    /// Walk pages back until the newest-oldest cursor passes `target_ms`
    /// (the original loads yesterday-00:00 on first run, then one more day per "load more").
    pub fn records_back_to(
        &self,
        from_next: &str,
        target_ms: i64,
        group_id: i64,
        team_id: i64,
    ) -> Result<(Vec<LiveItem>, String), String> {
        let mut next = from_next.to_string();
        let mut acc: Vec<LiveItem> = Vec::new();
        loop {
            if next == "0" && !acc.is_empty() {
                break;
            }
            let (items, cursor) = self.record_page_group(&next, group_id, team_id)?;
            let reached = items.last().and_then(|i| i.ctime).unwrap_or(i64::MAX) < target_ms;
            acc.extend(items);
            next = cursor;
            if reached || next == "0" {
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

    /// The address for a room, resolved the way the app does it: an explicit清晰度,
    /// then the merged multi-CDN URL, then the plain path.
    pub fn stream_url(
        &self,
        live_id: &str,
        source: Source,
        want_type: Option<i64>,
    ) -> Result<String, String> {
        let info = self.room_info(live_id, source)?;
        models::pick_stream(&info, want_type)
    }

    /// The whole room detail, which also feeds the人气值/公告/公演 numbers.
    pub fn room_detail(&self, live_id: &str, source: Source) -> Result<RoomInfo, String> {
        self.room_info(live_id, source)
    }

    /// The applause ranking of a theatre show. Anonymous, one parameter.
    pub fn open_member_rank(&self, live_id: &str) -> Result<Vec<models::MemberRank>, String> {
        let json = self.text(
            "getOpenLiveMemberRank",
            &serde_json::json!({ "liveId": live_id }),
        )?;
        models::parse_member_rank(&json)
    }

    /// Every room that is on air right now, in one call.
    pub fn ongoing_lives(&self) -> Result<Vec<models::OngoingLive>, String> {
        let json = self.text("get/all/ongoing/live/info", &serde_json::json!({}))?;
        models::parse_ongoing(&json)
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
