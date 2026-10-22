use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, TimeZone};
use egui::{Context, Rect};

use crate::api::Api;
use crate::config::{self, Config};
use crate::events::{Action, DownloadEvent, Event, ToastKind};
use crate::images::ImageCache;
use crate::player::{Entry, Player};
use crate::vlc::{Engine, Recorder};
use crate::models::{badge_text, LiveItem, Source};
use crate::theme as t;

const DAY_MS: i64 = 86_400_000;
/// How long a dropped recording waits before asking the server for a fresh address,
/// which is how the original client rides out a member reconnecting.
const RESUME_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Live,
    Record,
    Open,
    OpenRecord,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Live => "直播",
            Tab::Record => "录播",
            Tab::Open => "公演",
            Tab::OpenRecord => "公演录播",
        }
    }
    pub fn is_open(self) -> bool {
        matches!(self, Tab::Open | Tab::OpenRecord)
    }
    pub fn is_record(self) -> bool {
        matches!(self, Tab::Record | Tab::OpenRecord)
    }
    pub fn source(self) -> Source {
        if self.is_open() {
            Source::Open
        } else {
            Source::Member
        }
    }
}

/// Which overlay drawer is on screen.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fly {
    Settings,
    Filter,
}

#[derive(Clone, PartialEq)]
pub enum PlayerMode {
    System,
    Vlc,
    Builtin,
}

impl PlayerMode {
    fn from_cfg(s: &str) -> Self {
        match s {
            "vlc" => PlayerMode::Vlc,
            "builtin" => PlayerMode::Builtin,
            _ => PlayerMode::System,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            PlayerMode::System => "system",
            PlayerMode::Vlc => "vlc",
            PlayerMode::Builtin => "builtin",
        }
    }
}

/// Cheap handle threads use to talk back to the UI.
#[derive(Clone)]
pub struct Requester {
    ctx: Context,
    tx: Sender<Event>,
}

impl Requester {
    pub fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }
    pub fn tx(&self) -> Sender<Event> {
        self.tx.clone()
    }
    pub fn ctx(&self) -> &Context {
        &self.ctx
    }
    pub fn spawn<F: FnOnce(Requester) + Send + 'static>(&self, f: F) {
        let this = self.clone();
        std::thread::spawn(move || f(this));
    }
}

#[derive(Default)]
pub struct VlcStatus {
    pub checked: bool,
    pub installed: bool,
    pub integrated: bool,
    pub detail: String,
    pub progress: Option<(f64, String)>,
}

/// One running capture. `resume_at` is set when the recorder stops on its own: the
/// live may simply have ended, so it is only worth another address lookup once.
pub struct Recording {
    pub item: LiveItem,
    pub recorder: Recorder,
    pub url: String,
    pub started_at: Instant,
    pub resume_at: Option<Instant>,
    pub takes: u32,
}

pub struct App {
    pub req: Requester,
    rx: mpsc::Receiver<Event>,

    pub tab: Tab,
    pub lives: Vec<LiveItem>,
    pub records: Vec<LiveItem>,
    pub record_next: String,
    pub target_ms: i64,
    pub loading_live: bool,
    pub loading_records: bool,
    pub open_lives: Vec<LiveItem>,
    pub open_records: Vec<LiveItem>,
    pub open_next: String,
    pub open_record_next: String,
    pub loading_open: bool,
    /// Server-side "only this member's records"; a nickname is enough to resolve the id.
    pub member_only: Option<(String, String)>,

    pub search: String,
    pub groups: Vec<String>,
    pub types: Vec<String>,
    pub members: Vec<String>,
    pub date_on: bool,
    pub date_from: NaiveDate,
    pub date_to: NaiveDate,

    pub filter_open: bool,
    pub settings_open: bool,
    /// Which drawer is on screen - kept while it slides out - and its 0..1 progress.
    pub fly: Option<Fly>,
    pub fly_t: f32,
    pub fly_prev: f64,
    /// Measured grid row height, tagged with the column width it was measured at.
    pub row_height: Option<(f32, f32)>,
    /// Decoded covers waiting to become textures; uploading them all in one pass is
    /// what made scrolling stutter while a page of thumbnails arrived.
    pub img_pending: Vec<(String, Option<(usize, usize, Vec<u8>)>)>,
    pub member_query: String,
    pub preset_name: String,

    pub downloads: HashMap<String, f64>,
    /// url + destination per running record download, so "重试" can refill the holes.
    pub jobs: HashMap<String, (String, String)>,
    pub cancels: HashMap<String, Arc<AtomicBool>>,
    pub recordings: HashMap<String, Recording>,
    /// The next live-list arrival came from the grab timer, so match it against the
    /// watch list.
    pub grab_pending: bool,
    /// Chosen清晰度 per item; absent means "best non-RTMP".
    pub qualities: HashMap<String, i64>,
    pub grab_at: Option<Instant>,

    pub cfg: Config,
    pub mode: PlayerMode,
    pub images: ImageCache,
    pub toasts: Vec<(ToastKind, String, Instant)>,
    pub vlc: VlcStatus,
    pub vlc_check_pending: bool,
    pub chrome_done: bool,

    pub engine: Option<Arc<Engine>>,
    pub engine_loading: bool,
    pub pending_index: Option<usize>,
    pub pending_record: Option<String>,
    pub player: Option<Arc<Mutex<Player>>>,
    pub mini: Option<Arc<crate::mini::MiniState>>,
    pub playlist: Vec<Entry>,
    pub playlist_live: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        t::apply(&cc.egui_ctx);
        let (tx, rx) = mpsc::channel();
        let cfg = config::load();
        let mode = PlayerMode::from_cfg(cfg.player_mode.as_deref().unwrap_or("system"));

        let mut app = App {
            req: Requester {
                ctx: cc.egui_ctx.clone(),
                tx,
            },
            rx,
            tab: Tab::Live,
            filter_open: false,
            settings_open: false,
            fly: None,
            fly_t: 0.0,
            fly_prev: 0.0,
            row_height: None,
            img_pending: Vec::new(),
            lives: Vec::new(),
            records: Vec::new(),
            record_next: "0".to_owned(),
            target_ms: start_of_today_ms() - DAY_MS,
            loading_live: false,
            loading_records: false,
            open_lives: Vec::new(),
            open_records: Vec::new(),
            open_next: "0".to_owned(),
            open_record_next: "0".to_owned(),
            loading_open: false,
            member_only: None,
            search: String::new(),
            groups: Vec::new(),
            types: Vec::new(),
            members: Vec::new(),
            date_on: false,
            date_from: today(),
            date_to: today(),
            member_query: String::new(),
            preset_name: String::new(),
            downloads: HashMap::new(),
            jobs: HashMap::new(),
            cancels: HashMap::new(),
            recordings: HashMap::new(),
            grab_pending: false,
            qualities: HashMap::new(),
            grab_at: None,
            cfg,
            mode,
            images: ImageCache::default(),
            toasts: Vec::new(),
            vlc: VlcStatus::default(),
            vlc_check_pending: false,
            chrome_done: false,
            engine: None,
            engine_loading: false,
            pending_index: None,
            pending_record: None,
            player: None,
            mini: None,
            playlist: Vec::new(),
            playlist_live: true,
        };

        if app.mode == PlayerMode::Builtin {
            app.ensure_engine();
        }
        app.check_vlc();
        app.refresh_live();
        app.refresh_records(false);
        app
    }

    // ---------------------------------------------------------------- data

    pub fn refresh_live(&mut self) {
        if self.loading_live {
            return;
        }
        self.loading_live = true;
        let req = self.req.clone();
        req.spawn(move |r| {
            let api = Api::new();
            r.send(Event::LiveList(api.live_list()));
        });
    }

    pub fn refresh_records(&mut self, append: bool) {
        if self.loading_records {
            return;
        }
        self.loading_records = true;
        let req = self.req.clone();
        let next = if append {
            self.record_next.clone()
        } else {
            "0".to_owned()
        };
        let target = self.target_ms;
        let only = self.member_only.clone();
        req.spawn(move |r| {
            let api = Api::new();
            let res = match &only {
                Some((id, _)) if !append => api.records_for_user(id, target),
                Some((id, _)) => api
                    .record_page_user(&next, id)
                    .map(|(items, cursor)| (items, cursor)),
                None => api.records_back_to(&next, target),
            };
            r.send(Event::Records {
                result: res,
                append,
            });
        });
    }

    pub fn load_more(&mut self) {
        if self.record_next == "0" || self.loading_records {
            return;
        }
        self.target_ms = start_of_day_ms(self.target_ms) - DAY_MS;
        self.refresh_records(true);
    }

    pub fn refresh_open(&mut self, append: bool) {
        let record = self.tab == Tab::OpenRecord;
        if self.loading_open {
            return;
        }
        self.loading_open = true;
        let req = self.req.clone();
        let next = if record {
            self.open_record_next.clone()
        } else {
            self.open_next.clone()
        };
        let next = if append { next } else { "0".to_owned() };
        // One group selected means the server can do the narrowing for us.
        let group = match self.groups.as_slice() {
            [only] => crate::api::group_id(only).unwrap_or(0),
            _ => 0,
        };
        req.spawn(move |r| {
            let api = Api::new();
            r.send(Event::OpenList {
                result: api.open_live_list(record, &next, group),
                record,
                append,
            });
        });
    }

    /// Switches the record list to one member. The server needs a real cursor for a
    /// `userId` query, so the lookup happens on the worker thread.
    pub fn records_of(&mut self, nickname: &str) {
        let target = self.target_ms;
        let Some(id) = self
            .records
            .iter()
            .chain(self.lives.iter())
            .find(|i| i.nickname == nickname)
            .map(|i| i.user_id.clone())
            .filter(|id| !id.is_empty())
        else {
            self.toast(ToastKind::Warning, "没找到该成员的编号，先用搜索定位一次");
            return;
        };
        self.member_only = Some((id.clone(), nickname.to_owned()));
        self.tab = Tab::Record;
        self.loading_records = true;
        let req = self.req.clone();
        req.spawn(move |r| {
            let api = Api::new();
            let res = api.records_for_user(&id, target);
            r.send(Event::Records {
                result: res,
                append: false,
            });
        });
    }

    pub fn clear_member_only(&mut self) {
        self.member_only = None;
        self.refresh_records(false);
    }

    pub fn filtered(&self, tab: Tab) -> Vec<LiveItem> {
        let source = match tab {
            Tab::Live => &self.lives,
            Tab::Record => &self.records,
            Tab::Open => &self.open_lives,
            Tab::OpenRecord => &self.open_records,
        };
        let q = self.search.to_lowercase();
        let from = if self.date_on {
            Some(self.date_from.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis())
        } else {
            None
        };
        let to = if self.date_on {
            Some(self.date_to.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp_millis())
        } else {
            None
        };
        let member = tab.source() == Source::Member;

        source
            .iter()
            .filter(|i| {
                if tab.is_record() && !tab.is_open() {
                    if let Some(c) = i.ctime {
                        if c < self.target_ms {
                            return false;
                        }
                        if let Some(f) = from {
                            if c < f {
                                return false;
                            }
                        }
                        if let Some(f) = to {
                            if c > f {
                                return false;
                            }
                        }
                    }
                }
                if !q.is_empty()
                    && !i.nickname.to_lowercase().contains(&q)
                    && !i.title.to_lowercase().contains(&q)
                    && !i.sub_title.to_lowercase().contains(&q)
                {
                    return false;
                }
                // Group, type and member filters describe a person's room; a theatre
                // show has none of those attributes, so they must not hide it.
                if member && !self.groups.is_empty() {
                    if !self
                        .groups
                        .iter()
                        .any(|g| i.nickname.to_uppercase().starts_with(g))
                    {
                        return false;
                    }
                }
                if member && !self.types.is_empty() {
                    let badge = badge_text(i);
                    let linked = i.in_microphone_connection;
                    let hit = self.types.iter().any(|ty| {
                        if ty == "连麦" {
                            linked || badge == "连麦"
                        } else {
                            badge == ty
                        }
                    });
                    if !hit {
                        return false;
                    }
                }
                if member && !self.members.is_empty() && !self.members.contains(&i.nickname) {
                    return false;
                }
                true
            })
            .cloned()
            .collect()
    }

    pub fn nicknames(&self) -> Vec<String> {
        let source = match self.tab {
            Tab::Live => &self.lives,
            Tab::Record => &self.records,
            Tab::Open | Tab::OpenRecord => return Vec::new(),
        };
        let mut names: Vec<String> = source.iter().map(|i| i.nickname.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn has_filters(&self) -> bool {
        !self.groups.is_empty()
            || !self.types.is_empty()
            || !self.members.is_empty()
            || self.date_on
            || self.member_only.is_some()
    }

    // -------------------------------------------------------------- actions

    pub fn act(&mut self, item: &LiveItem, action: Action) {
        let req = self.req.clone();
        let live_id = item.live_id.clone();
        let source = item.source;
        req.spawn(move |r| {
            let api = Api::new();
            let res = api.playable_stream_url(&live_id, source, None);
            r.send(Event::StreamUrl {
                result: res,
                action: Box::new(action),
            });
        });
    }

    /// Fills in the清晰度 menu and the danmaku address for the item that just opened.
    pub fn fetch_streams(&mut self, live_id: String, source: Source) {
        let req = self.req.clone();
        req.spawn(move |r| {
            let api = Api::new();
            let res = api
                .room_info(&live_id, source)
                .map(|info| (info.streams, info.msg_file_path));
            r.send(Event::Streams {
                live_id,
                result: res,
            });
        });
    }

    pub fn fetch_danmu(&mut self, live_id: String, url: String) {
        let req = self.req.clone();
        req.spawn(move |r| {
            let api = Api::new();
            r.send(Event::Danmu {
                live_id,
                result: api.danmaku(&url),
            });
        });
    }

    /// Saves a record's `.lrc` next to the configured download directory.
    pub fn save_danmu(&mut self, item: &LiveItem, url: String) {
        let req = self.req.clone();
        let name = file_name(item, "口袋48弹幕", "lrc");
        let dir = self.download_dir();
        req.spawn(move |r| {
            let api = Api::new();
            let _ = std::fs::create_dir_all(&dir);
            let path = std::path::Path::new(&dir).join(&name);
            let res = crate::download::save_text(&api, &url, &path.to_string_lossy())
                .map(|_| path.to_string_lossy().to_string());
            r.send(Event::DanmuSaved(res));
        });
    }

    pub fn download_dir(&self) -> String {
        self.cfg
            .download_dir
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| config::dir().join("downloads").to_string_lossy().to_string())
    }

    pub fn cancel_download(&mut self, live_id: &str) {
        if let Some(flag) = self.cancels.remove(live_id) {
            flag.store(true, Ordering::SeqCst);
        }
        self.downloads.remove(live_id);
        self.toast(ToastKind::Info, "已发送取消下载请求");
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toasts.push((kind, text.into(), Instant::now()));
        if self.toasts.len() > 4 {
            let overflow = self.toasts.len() - 4;
            self.toasts.rotate_left(overflow);
            self.toasts.truncate(4);
        }
        self.req.ctx().request_repaint();
    }


    // ---------------------------------------------------------------- engine

    pub fn ensure_engine(&mut self) {
        if self.engine.is_some() || self.engine_loading {
            return;
        }
        self.engine_loading = true;
        let req = self.req.clone();
        req.spawn(move |r| r.send(Event::EngineReady(Engine::load().map(Arc::new))));
    }

    pub fn download_engine(&mut self) {
        if self.vlc.progress.is_some() {
            return;
        }
        self.vlc.progress = Some((0.0, "准备下载…".to_owned()));
        crate::vlc::spawn_download(self.req.clone());
    }

    pub fn check_vlc(&mut self) {
        let req = self.req.clone();
        req.spawn(move |r| {
            let st = crate::vlc::status();
            r.send(Event::VlcCheck {
                installed: st.loadable,
                integrated: st.bundled,
                detail: st.detail,
            });
        });
    }

    pub fn current_entries(&self) -> (Vec<Entry>, bool) {
        let tab = self.tab;
        let is_live = tab == Tab::Live || tab == Tab::Open;
        let entries = self
            .filtered(tab)
            .iter()
            .map(|i| Entry {
                live_id: i.live_id.clone(),
                title: i.title.clone(),
                nickname: if i.nickname.is_empty() {
                    i.sub_title.clone()
                } else {
                    i.nickname.clone()
                },
                cover: crate::api::resolve_image(&i.cover_path),
                source: i.source,
            })
            .collect();
        (entries, is_live)
    }

    /// Where the playlist drawer currently sits, for wheel hit-testing.
    pub fn playlist_hit(&self) -> Rect {
        self.player.as_ref().map_or(Rect::NOTHING, |p| {
            p.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
                .playlist_rect
        })
    }

    pub fn start_builtin(&mut self, live_id: &str) {
        let (entries, is_live) = self.current_entries();
        let index = entries
            .iter()
            .position(|e| e.live_id == live_id)
            .unwrap_or(0);
        self.playlist = entries;
        self.playlist_live = is_live;
        self.ensure_engine();
        self.resolve_builtin(index);
    }

    pub fn resolve_builtin(&mut self, index: usize) {
        let Some(entry) = self.playlist.get(index).cloned() else {
            return;
        };
        if self.engine.is_none() {
            self.pending_index = Some(index);
            self.ensure_engine();
            return;
        }
        let req = self.req.clone();
        let live_id = entry.live_id.clone();
        let source = entry.source;
        let want = self.qualities.get(&live_id).copied();
        let probe = live_id.clone();
        req.spawn(move |r| {
            let api = Api::new();
            // libVLC demuxes RTMP itself, so the raw playStreamPath is preferred here.
            let res = api
                .stream_url(&probe, source, want)
                .or_else(|_| api.playable_stream_url(&probe, source, want));
            r.send(Event::StreamUrl {
                result: res,
                action: Box::new(Action::OpenBuiltin { index }),
            });
        });
        self.fetch_streams(live_id, source);
    }

    fn open_builtin(&mut self, index: usize, url: String) {
        let Some(engine) = self.engine.clone() else {
            self.pending_index = Some(index);
            self.ensure_engine();
            return;
        };
        let ctx = self.req.ctx().clone();
        let existing = self.player.clone();
        if let Some(arc) = existing {
            let result = arc
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .reattach(&engine, &url, index);
            if let Err(e) = result {
                self.toast(ToastKind::Error, format!("内置播放失败：{e}"));
            }
        } else {
            match Player::open(
                &engine,
                &url,
                self.playlist.clone(),
                index,
                self.playlist_live,
                &ctx,
            ) {
                Ok(player) => self.player = Some(Arc::new(Mutex::new(player))),
                Err(e) => self.toast(ToastKind::Error, format!("内置播放失败：{e}")),
            }
        }
    }

    pub fn handle_cmd(&mut self, cmd: crate::player::Cmd) {
        use crate::player::Cmd;
        match cmd {
            Cmd::None => {}
            Cmd::Close => {
                self.player = None;
            }
            Cmd::Refresh => {
                if let Some(arc) = &self.player {
                    let index = arc
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .index;
                    self.resolve_builtin(index);
                }
            }
            Cmd::Prev | Cmd::Next => {
                let Some(arc) = &self.player else { return };
                let (index, len, entries, is_live) = {
                    let p = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    (p.index, p.entries.len(), p.entries.clone(), p.is_live)
                };
                let next = if cmd == Cmd::Next { index + 1 } else { index.saturating_sub(1) };
                if next >= len {
                    let msg = if cmd == Cmd::Next { "已经是最后一个了" } else { "已经是第一个了" };
                    self.toast(ToastKind::Info, msg);
                    return;
                }
                self.playlist = entries;
                self.playlist_live = is_live;
                self.resolve_builtin(next);
            }
            Cmd::Goto(i) => {
                if let Some(arc) = &self.player {
                    let (entries, is_live) = {
                        let p = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        (p.entries.clone(), p.is_live)
                    };
                    self.playlist = entries;
                    self.playlist_live = is_live;
                    self.resolve_builtin(i);
                }
            }
            Cmd::Mini => {
                let Some(player) = self.player.clone() else { return };
                self.mini = Some(Arc::new(crate::mini::MiniState::new(player)));
                // Same behaviour as the original: the main window steps aside.
                self.req
                    .ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            Cmd::Quality(t) => {
                let Some(arc) = &self.player else { return };
                let (index, live_id) = {
                    let p = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    match p.entries.get(p.index) {
                        Some(e) => (p.index, e.live_id.clone()),
                        None => return,
                    }
                };
                self.qualities.insert(live_id, t);
                self.resolve_builtin(index);
            }
            Cmd::Record => {
                let Some(arc) = &self.player else { return };
                let live_id = {
                    let p = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    p.entries.get(p.index).map(|e| e.live_id.clone())
                };
                if let Some(id) = live_id {
                    self.toggle_recording(&id);
                }
            }
            Cmd::SaveDanmu => {
                let Some(arc) = self.player.clone() else { return };
                let found = {
                    let guard = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard
                        .entries
                        .get(guard.index)
                        .and_then(|e| guard.danmu_url.clone().map(|u| (e.live_id.clone(), u)))
                };
                match found.and_then(|(id, url)| self.find_item(&id).map(|item| (item, url))) {
                    Some((item, url)) => self.save_danmu(&item, url),
                    None => self.toast(ToastKind::Warning, "这条直播没有弹幕文件"),
                }
            }
        }
    }

    // ------------------------------------------------------------- recording

    pub fn find_item(&self, live_id: &str) -> Option<LiveItem> {
        self.lives
            .iter()
            .chain(self.records.iter())
            .chain(self.open_lives.iter())
            .chain(self.open_records.iter())
            .find(|i| i.live_id == live_id)
            .cloned()
    }

    pub fn toggle_recording(&mut self, live_id: &str) {
        if self.recordings.contains_key(live_id) {
            self.stop_recording(live_id);
            return;
        }
        let Some(item) = self.find_item(live_id) else {
            self.toast(ToastKind::Error, "找不到这条直播，请先刷新列表");
            return;
        };
        let Some(engine) = self.engine.clone() else {
            self.ensure_engine();
            self.pending_record = Some(live_id.to_owned());
            self.toast(ToastKind::Info, "正在准备 libVLC 引擎，稍后自动开始录制");
            return;
        };
        let dir = self.download_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.toast(ToastKind::Error, format!("无法创建录制目录：{e}"));
            return;
        }
        let kind = if item.source == Source::Open { "48公演直播" } else { "口袋48直播" };
        let path = std::path::Path::new(&dir).join(file_name(&item, kind, "ts"));
        self.start_capture(item, engine, path.to_string_lossy().to_string());
    }

    fn start_capture(&mut self, item: LiveItem, engine: Arc<Engine>, path: String) {
        let req = self.req.clone();
        let live_id = item.live_id.clone();
        let source = item.source;
        req.spawn(move |r| {
            let api = Api::new();
            let res = api
                .stream_url(&live_id, source, None)
                .or_else(|_| api.playable_stream_url(&live_id, source, None))
                .and_then(|url| engine.record(&url, &path).map(|rec| (url, rec)));
            r.send(Event::RecordStarted { live_id, result: res });
        });
        self.toast(ToastKind::Info, "开始录制");
    }

    pub fn stop_recording(&mut self, live_id: &str) {
        if let Some(rec) = self.recordings.remove(live_id) {
            let secs = rec.started_at.elapsed().as_secs();
            self.toast(
                ToastKind::Success,
                format!("已停止录制（{} 秒）→ {}", secs, rec.recorder.out),
            );
        }
    }

    /// A stream that dies mid-recording is usually the member reconnecting: wait a
    /// minute, ask for the address again, and only keep capturing if it actually moved.
    fn tick_recorders(&mut self) {
        let mut restart: Vec<(String, LiveItem, String)> = Vec::new();
        let now = Instant::now();
        for (id, rec) in self.recordings.iter_mut() {
            if rec.recorder.running() {
                continue;
            }
            if rec.resume_at.is_none() {
                rec.resume_at = Some(now + RESUME_AFTER);
                continue;
            }
            if let Some(at) = rec.resume_at {
                if now < at {
                    continue;
                }
                rec.resume_at = Some(now + RESUME_AFTER);
                restart.push((id.clone(), rec.item.clone(), rec.url.clone()));
            }
        }
        for (id, item, previous) in restart {
            let Some(engine) = self.engine.clone() else { continue };
            let req = self.req.clone();
            let live_id = id.clone();
            let dir = self.download_dir();
            let kind = if item.source == Source::Open { "48公演直播" } else { "口袋48直播" };
            let base = item.clone();
            req.spawn(move |r| {
                let api = Api::new();
                let url = api
                    .stream_url(&live_id, base.source, None)
                    .or_else(|_| api.playable_stream_url(&live_id, base.source, None));
                match url {
                    Err(_) => r.send(Event::RecordStopped { live_id }),
                    Ok(u) if u == previous => r.send(Event::RecordStopped { live_id }),
                    Ok(u) => {
                        let path = next_take(&dir, &base, kind);
                        let res = engine.record(&u, &path).map(|rec| (u, rec));
                        r.send(Event::RecordStarted { live_id, result: res });
                    }
                }
            });
        }
    }

    /// Auto grab: poll the live list on a timer and capture whoever on the watch list
    /// is on air. Recording stays capped so the source does not get hammered.
    pub fn tick_grab(&mut self) {
        let g = self.cfg.grab.clone();
        if !g.enabled || g.users.trim().is_empty() {
            self.grab_at = None;
            return;
        }
        if self.engine.is_none() {
            self.ensure_engine();
            return;
        }
        let every = Duration::from_secs(g.minutes.clamp(1, 720) * 60);
        let due = self.grab_at.map_or(true, |at| Instant::now() >= at);
        if !due || self.loading_live {
            return;
        }
        self.grab_at = Some(Instant::now() + every);
        self.refresh_live();
        self.grab_pending = true;
    }

    /// Called with the fresh live list whenever a grab round's request comes back.
    fn grab_match(&mut self, items: &[LiveItem]) {
        let g = self.cfg.grab.clone();
        if !g.enabled {
            return;
        }
        let names: Vec<String> = g
            .users
            .split([',', '，', ' ', '\n', '\t'])
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            return;
        }
        let dir = if g.dir.trim().is_empty() {
            self.download_dir()
        } else {
            g.dir.clone()
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.toast(ToastKind::Error, format!("无法创建抓取目录：{e}"));
            return;
        }
        let mut slots = g.max.saturating_sub(self.recordings.len());
        for item in items {
            if slots == 0 {
                break;
            }
            let hit = names.iter().any(|n| {
                item.nickname.to_lowercase().contains(n) || item.user_id == *n
            });
            if !hit || self.recordings.contains_key(&item.live_id) {
                continue;
            }
            let path = next_take(&dir, item, "口袋48直播(自动抓取)");
            let Some(engine) = self.engine.clone() else { return };
            append_grab_log(&dir, item, &path);
            self.start_capture(item.clone(), engine, path);
            slots -= 1;
        }
    }

    // ---------------------------------------------------------------- render

    pub fn drain_events(&mut self) {
        // Upload a few textures per pass at most: a burst of 760px uploads in one
        // frame is what turned into a visible hitch while scrolling.
        for _ in 0..3 {
            if self.img_pending.is_empty() {
                break;
            }
            let (url, result) = self.img_pending.remove(0);
            self.images.apply_loaded(&url, result, self.req.ctx());
        }
        if !self.img_pending.is_empty() {
            self.req
                .ctx()
                .request_repaint_after(std::time::Duration::from_millis(8));
        }
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::LiveList(Ok(items)) => {
                    if self.grab_pending {
                        self.grab_pending = false;
                        self.grab_match(&items);
                    }
                    self.lives = items;
                    self.loading_live = false;
                }
                Event::LiveList(Err(e)) => {
                    self.loading_live = false;
                    self.toast(ToastKind::Error, format!("获取直播列表失败：{e}"));
                }
                Event::Records { result, append } => match result {
                    Ok((items, next)) => {
                        if append {
                            self.records.extend(items);
                        } else {
                            self.records = items;
                        }
                        self.record_next = next;
                        self.loading_records = false;
                    }
                    Err(e) => {
                        self.loading_records = false;
                        self.toast(ToastKind::Error, format!("获取录播列表失败：{e}"));
                    }
                },
                Event::Image { url, result } => {
                    self.img_pending.push((url, result));
                }
                Event::StreamUrl { result, action } => match result {
                    Err(e) => self.toast(ToastKind::Error, format!("获取播放地址失败：{e}")),
                    Ok(url) => match *action {
                        Action::Copy => {
                            self.req.ctx().copy_text(url);
                            self.toast(ToastKind::Success, "已复制推流/录播地址");
                        }
                        Action::Download { item } => self.save_dialog(url, item),
                        Action::PlayExternal => self.open_in_player(url),
                        Action::PlayVlcExternal => self.open_in_vlc(url),
                        Action::OpenBuiltin { index } => self.open_builtin(index, url),
                    },
                },
                Event::OpenList {
                    result,
                    record,
                    append,
                } => match result {
                    Ok((items, next)) => {
                        if record {
                            if append {
                                self.open_records.extend(items);
                            } else {
                                self.open_records = items;
                            }
                            self.open_record_next = next;
                        } else {
                            if append {
                                self.open_lives.extend(items);
                            } else {
                                self.open_lives = items;
                            }
                            self.open_next = next;
                        }
                        self.loading_open = false;
                    }
                    Err(e) => {
                        self.loading_open = false;
                        self.toast(ToastKind::Error, format!("获取公演列表失败：{e}"));
                    }
                },
                Event::Streams {
                    live_id,
                    result: Ok((streams, msg)),
                } => {
                    if let Some(arc) = &self.player {
                        let want = self.qualities.get(&live_id).copied();
                        let mut p = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        if p.entries.get(p.index).map(|e| e.live_id.as_str()) == Some(&live_id) {
                            p.set_streams(streams, want);
                            p.danmu_url = msg;
                        }
                    }
                }
                Event::Streams {
                    live_id: _,
                    result: Err(_),
                } => {}
                Event::Danmu { live_id, result } => match result {
                    Ok(items) => {
                        if let Some(arc) = &self.player {
                            let mut p = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                            if p.entries.get(p.index).map(|e| e.live_id.as_str()) == Some(&live_id)
                            {
                                p.set_danmu(items);
                            }
                        }
                    }
                    Err(e) => self.toast(ToastKind::Warning, format!("弹幕加载失败：{e}")),
                },
                Event::DanmuSaved(result) => match result {
                    Ok(path) => self.toast(ToastKind::Success, format!("弹幕已保存：{path}")),
                    Err(e) => self.toast(ToastKind::Error, format!("弹幕保存失败：{e}")),
                },
                Event::RecordStarted { live_id, result } => match result {
                    Ok((url, recorder)) => {
                        let takes = self
                            .recordings
                            .get(&live_id)
                            .map(|r| r.takes + 1)
                            .unwrap_or(0);
                        match self.find_item(&live_id) {
                            Some(item) => {
                                let out = recorder.out.clone();
                                self.recordings.insert(
                                    live_id,
                                    Recording {
                                        item,
                                        recorder,
                                        url,
                                        started_at: Instant::now(),
                                        resume_at: None,
                                        takes,
                                    },
                                );
                                if takes > 0 {
                                    self.toast(
                                        ToastKind::Warning,
                                        format!("流地址变了，续录到 {out}"),
                                    );
                                }
                            }
                            None => {
                                drop(recorder);
                                self.toast(ToastKind::Error, "找不到这条直播，录制已停止");
                            }
                        }
                        self.req
                            .ctx()
                            .request_repaint_after(Duration::from_millis(500));
                    }
                    Err(e) => {
                        self.recordings.remove(&live_id);
                        self.toast(ToastKind::Error, format!("录制失败：{e}"));
                    }
                },
                Event::RecordStopped { live_id } => {
                    if let Some(rec) = self.recordings.remove(&live_id) {
                        self.toast(
                            ToastKind::Info,
                            format!("直播已结束，录制停止 → {}", rec.recorder.out),
                        );
                    }
                }
                Event::Download { live_id, kind } => match kind {
                    DownloadEvent::Progress(p) => {
                        self.downloads.insert(live_id, p);
                    }
                    DownloadEvent::Success => {
                        self.downloads.remove(&live_id);
                        self.cancels.remove(&live_id);
                        self.jobs.remove(&live_id);
                        self.toast(ToastKind::Success, "下载完成");
                    }
                    DownloadEvent::Error(e) => {
                        self.downloads.remove(&live_id);
                        self.cancels.remove(&live_id);
                        // Keep the job so the retry button can resume from the cache.
                        self.toast(ToastKind::Error, format!("下载失败：{e}（可重试续传）"));
                    }
                    DownloadEvent::Cancelled => {
                        self.downloads.remove(&live_id);
                        self.toast(ToastKind::Warning, "下载已取消");
                    }
                },
                Event::VlcCheck {
                    installed,
                    integrated,
                    detail,
                } => {
                    self.vlc_check_pending = false;
                    self.vlc.checked = true;
                    self.vlc.installed = installed;
                    self.vlc.integrated = integrated;
                    self.vlc.detail = detail;
                }
                Event::VlcDownload(kind) => match kind {
                    DownloadEvent::Progress(p) => {
                        self.vlc.progress = Some((p, format!("{p:.1}%")));
                    }
                    DownloadEvent::Success => {
                        self.vlc.progress = None;
                        self.vlc.integrated = true;
                        self.vlc.installed = true;
                        self.toast(ToastKind::Success, "VLC 引擎下载完成");
                        self.check_vlc();
                        self.ensure_engine();
                    }
                    DownloadEvent::Error(e) => {
                        self.vlc.progress = Some((-1.0, e));
                    }
                    DownloadEvent::Cancelled => self.vlc.progress = None,
                },
                Event::EngineReady(Ok(engine)) => {
                    self.engine_loading = false;
                    self.engine = Some(engine);
                    if let Some(index) = self.pending_index.take() {
                        self.resolve_builtin(index);
                    }
                    if let Some(live_id) = self.pending_record.take() {
                        self.toggle_recording(&live_id);
                    }
                }
                Event::EngineReady(Err(e)) => {
                    self.engine_loading = false;
                    self.toast(ToastKind::Error, format!("libVLC 引擎加载失败：{e}"));
                }
                Event::ExternalPlayer(Err(e)) => {
                    self.toast(ToastKind::Error, format!("启动播放器失败：{e}"))
                }
                Event::ExternalPlayer(Ok(())) => {}
            }
        }
    }

    fn save_dialog(&mut self, url: String, item: LiveItem) {
        let dir = self.download_dir();
        let kind = if item.source == Source::Open { "48公演录播" } else { "口袋48录播" };
        let suggested = std::path::Path::new(&dir).join(file_name(&item, kind, "ts"));
        let file = rfd::FileDialog::new()
            .set_file_name(suggested.file_name().and_then(|s| s.to_str()).unwrap_or("record.ts"))
            .add_filter("视频文件", &["ts", "mp4", "flv"])
            .save_file();
        let Some(path) = file else { return };
        let save_path = path.to_string_lossy().to_string();
        let live_id = item.live_id.clone();
        self.jobs.insert(live_id.clone(), (url.clone(), save_path.clone()));
        self.begin_download(live_id, url, save_path);
    }

    fn begin_download(&mut self, live_id: String, url: String, save_path: String) {
        if let Some(parent) = std::path::Path::new(&save_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let flag = Arc::new(AtomicBool::new(false));
        self.cancels.insert(live_id.clone(), flag.clone());
        let tx = self.req.tx();
        self.downloads.insert(live_id.clone(), 0.0);
        download_start(url, save_path, live_id, tx, flag);
    }

    /// Fills in only the pieces that are still missing from `<target>.cache/`.
    pub fn retry_download(&mut self, live_id: &str) {
        let Some((url, path)) = self.jobs.get(live_id).cloned() else {
            self.toast(ToastKind::Warning, "没有可续传的下载记录");
            return;
        };
        self.toast(ToastKind::Info, "重试缺失的分片");
        self.begin_download(live_id.to_owned(), url, path);
    }

    fn open_in_player(&self, url: String) {
        let req = self.req.clone();
        req.spawn(move |r| r.send(Event::ExternalPlayer(open_with_default_player(&url))));
    }

    fn open_in_vlc(&self, url: String) {
        let req = self.req.clone();
        req.spawn(move |r| r.send(Event::ExternalPlayer(open_in_vlc_player(&url))));
    }

    /// Keeps captures alive and the watch list polled; runs every frame from `logic`.
    pub fn tick_timers(&mut self) {
        self.tick_recorders();
        self.tick_grab();
        if !self.recordings.is_empty() || self.cfg.grab.enabled {
            self.req
                .ctx()
                .request_repaint_after(Duration::from_millis(1000));
        }
    }

    pub fn tick_toasts(&mut self) {
        let now = Instant::now();
        let before = self.toasts.len();
        self.toasts.retain(|(_, _, at)| now.duration_since(*at) < Duration::from_secs(4));
        if self.toasts.len() != before || !self.toasts.is_empty() {
            self.req
                .ctx()
                .request_repaint_after(Duration::from_millis(250));
        }
    }
}

// tiny helpers -------------------------------------------------------------

fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn start_of_today_ms() -> i64 {
    start_of_day_ms(Local::now().timestamp_millis())
}

fn start_of_day_ms(ms: i64) -> i64 {
    let d = Local.timestamp_millis_opt(ms).single().unwrap().date_naive();
    d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis()
}

pub fn fmt_time(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Every auto capture leaves a line in the directory it saved to, the way the original
/// client kept `pocket48_live_log.txt`.
fn append_grab_log(dir: &str, item: &LiveItem, path: &str) {
    use std::io::Write;
    let file = std::path::Path::new(dir).join("pocket48_live_log.txt");
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) else {
        return;
    };
    let started = item
        .ctime
        .map(crate::app::fmt_time)
        .unwrap_or_else(|| fmt_time(chrono::Local::now().timestamp_millis()));
    let _ = writeln!(
        f,
        "直播标题：{}\n直播ID：{}\n房间号：{}\n直播人：{}\n直播时间：{started}\n输出文件：{path}\n创建时间：{}\n",
        item.title,
        item.live_id,
        item.room_id,
        item.nickname,
        fmt_time(chrono::Local::now().timestamp_millis())
    );
}

/// What goes between `[` and `]` in a file name for this kind of capture.
pub fn file_name(item: &LiveItem, kind: &str, ext: &str) -> String {
    let started = item
        .ctime
        .map(file_time)
        .unwrap_or_else(|| file_time(chrono::Local::now().timestamp_millis()));
    let now = file_time(chrono::Local::now().timestamp_millis());
    let info = [
        sanitize(&item.nickname),
        sanitize(&item.title),
        started,
        item.live_id.clone(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("_");
    format!("[{kind}]_{info}__{now}.{ext}")
}

/// The original client's stamp: `~` and `.` keep it legal on every OS.
fn file_time(ms: i64) -> String {
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%Y-%m-%d~%H.%M.%S").to_string())
        .unwrap_or_default()
}

fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\t'))
        .collect()
}

/// `(1)`, `(2)`, … after the file stem, which is how a recording that survived a
/// reconnect is kept apart from the earlier take.
fn next_take(dir: &str, item: &LiveItem, kind: &str) -> String {
    let base = file_name(item, kind, "ts");
    let mut path = std::path::Path::new(dir).join(&base);
    loop {
        if !path.exists() {
            return path.to_string_lossy().to_string();
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("take").to_owned();
        let (head, paren) = match (stem.rfind('('), stem.rfind(')')) {
            (Some(a), Some(b)) if b > a => (&stem[..a], stem[a + 1..b].parse::<u32>().ok()),
            _ => (stem.as_str(), None),
        };
        let n = paren.map(|x| x + 1).unwrap_or(1);
        let name = format!("{head}({n}).ts");
        path = path.with_file_name(name);
    }
}

#[cfg(windows)]
fn open_with_default_player(url: &str) -> Result<(), String> {
    for exe in [
        "PotPlayer64",
        "PotPlayerMini64",
        r"C:\Program Files\DAUM\PotPlayer\PotPlayer64.exe",
        r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe",
    ] {
        if std::process::Command::new(exe).arg(url).spawn().is_ok() {
            return Ok(());
        }
    }
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &format!("Start-Process '{}'", url.replace('\'', "''")),
        ])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn open_with_default_player(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn open_in_vlc_player(url: &str) -> Result<(), String> {
    for exe in [
        "vlc",
        r"C:\Program Files\VideoLAN\VLC\vlc.exe",
        r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe",
    ] {
        if std::process::Command::new(exe).arg(url).spawn().is_ok() {
            return Ok(());
        }
    }
    Err("未找到 VLC 播放器".to_string())
}

#[cfg(not(windows))]
fn open_in_vlc_player(url: &str) -> Result<(), String> {
    std::process::Command::new("vlc")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn download_start(
    url: String,
    save_path: String,
    live_id: String,
    tx: Sender<Event>,
    cancelled: Arc<AtomicBool>,
) {
    crate::download::start(url, save_path, live_id, tx, cancelled);
}
