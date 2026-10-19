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
use crate::vlc::Engine;
use crate::models::{badge_text, LiveItem};
use crate::theme as t;

const DAY_MS: i64 = 86_400_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Live,
    Record,
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
    pub cancels: HashMap<String, Arc<AtomicBool>>,

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
            cancels: HashMap::new(),
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
        req.spawn(move |r| {
            let api = Api::new();
            let res = api.records_back_to(&next, target);
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

    pub fn filtered(&self, tab: Tab) -> Vec<LiveItem> {
        let source = if tab == Tab::Live { &self.lives } else { &self.records };
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

        source
            .iter()
            .filter(|i| {
                if tab == Tab::Record {
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
                {
                    return false;
                }
                if !self.groups.is_empty()
                    && !self
                        .groups
                        .iter()
                        .any(|g| i.nickname.to_uppercase().starts_with(g))
                {
                    return false;
                }
                if !self.types.is_empty() {
                    let badge = badge_text(i.live_type, i.in_microphone_connection);
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
                if !self.members.is_empty() && !self.members.contains(&i.nickname) {
                    return false;
                }
                true
            })
            .cloned()
            .collect()
    }

    pub fn nicknames(&self) -> Vec<String> {
        let source = if self.tab == Tab::Live { &self.lives } else { &self.records };
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
    }

    // -------------------------------------------------------------- actions

    pub fn act(&mut self, item: &LiveItem, action: Action) {
        let req = self.req.clone();
        let live_id = item.live_id.clone();
        req.spawn(move |r| {
            let api = Api::new();
            let res = api.playable_stream_url(&live_id);
            r.send(Event::StreamUrl {
                result: res,
                action: Box::new(action),
            });
        });
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
        let is_live = tab == Tab::Live;
        let entries = self
            .filtered(tab)
            .iter()
            .map(|i| Entry {
                live_id: i.live_id.clone(),
                title: i.title.clone(),
                nickname: i.nickname.clone(),
                cover: crate::api::resolve_image(&i.cover_path),
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
        req.spawn(move |r| {
            let api = Api::new();
            // libVLC demuxes RTMP itself, so the raw playStreamPath is preferred here.
            let res = api
                .stream_url(&entry.live_id)
                .or_else(|_| api.playable_stream_url(&entry.live_id));
            r.send(Event::StreamUrl {
                result: res,
                action: Box::new(Action::OpenBuiltin { index }),
            });
        });
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
                        Action::Download { live_id, title } => {
                            self.save_dialog(url, live_id, title)
                        }
                        Action::PlayExternal => self.open_in_player(url),
                        Action::PlayVlcExternal => self.open_in_vlc(url),
                        Action::OpenBuiltin { index } => self.open_builtin(index, url),
                    },
                },
                Event::Download { live_id, kind } => match kind {
                    DownloadEvent::Progress(p) => {
                        self.downloads.insert(live_id, p);
                    }
                    DownloadEvent::Success => {
                        self.downloads.remove(&live_id);
                        self.cancels.remove(&live_id);
                        self.toast(ToastKind::Success, "下载完成");
                    }
                    DownloadEvent::Error(e) => {
                        self.downloads.remove(&live_id);
                        self.cancels.remove(&live_id);
                        self.toast(ToastKind::Error, format!("下载失败：{e}"));
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

    fn save_dialog(&mut self, url: String, live_id: String, title: String) {
        let file = rfd::FileDialog::new()
            .set_file_name(default_file_name(&live_id, &title))
            .add_filter("视频文件", &["ts", "mp4", "flv"])
            .save_file();
        let Some(path) = file else { return };
        let save_path = path.to_string_lossy().to_string();
        let flag = Arc::new(AtomicBool::new(false));
        self.cancels.insert(live_id.clone(), flag.clone());
        let tx = self.req.tx();
        download_start(url, save_path, live_id, tx, flag);
    }

    fn open_in_player(&self, url: String) {
        let req = self.req.clone();
        req.spawn(move |r| r.send(Event::ExternalPlayer(open_with_default_player(&url))));
    }

    fn open_in_vlc(&self, url: String) {
        let req = self.req.clone();
        req.spawn(move |r| r.send(Event::ExternalPlayer(open_in_vlc_player(&url))));
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

fn default_file_name(live_id: &str, title: &str) -> String {
    let clean: String = title
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let stem: String = clean.chars().take(24).collect();
    if stem.is_empty() {
        format!("Live_{live_id}.ts")
    } else {
        format!("{stem}.ts")
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
