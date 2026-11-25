use std::sync::Arc;

use crate::models::{Danmu, LiveItem, MemberRank, OngoingLive, RoomInfo};
use crate::vlc::{Engine, Recorder};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
    Warning,
}

pub enum DownloadEvent {
    Progress(f64),
    Success,
    Error(String),
    Cancelled,
}

pub enum Event {
    LiveList(Result<Vec<LiveItem>, String>),
    Records {
        result: Result<(Vec<LiveItem>, String), String>,
        append: bool,
    },
    /// Theatre shows, live/scheduled (`record=false`) or archived (`record=true`).
    OpenList {
        result: Result<(Vec<LiveItem>, String), String>,
        record: bool,
        append: bool,
    },
    StreamUrl {
        result: Result<String, String>,
        action: Box<Action>,
    },
    /// The room detail behind the清晰度 menu: streams,人气值,公告,公演 numbers…
    Streams {
        live_id: String,
        result: Result<RoomInfo, String>,
    },
    /// The applause ranking of the theatre show that is on screen.
    Rank {
        live_id: String,
        result: Result<Vec<MemberRank>, String>,
    },
    /// Everything that is on air right now.
    Ongoing(Result<Vec<OngoingLive>, String>),
    Danmu {
        live_id: String,
        result: Result<Vec<Danmu>, String>,
    },
    RecordStarted {
        live_id: String,
        result: Result<(String, Recorder), String>,
    },
    /// The source did not come back, so the capture is over.
    RecordStopped { live_id: String },
    /// Where a record's `.lrc` danmaku ended up on disk.
    DanmuSaved(Result<String, String>),
    Image {
        url: String,
        result: Option<(usize, usize, Vec<u8>)>,
    },
    Download {
        live_id: String,
        kind: DownloadEvent,
    },
    VlcCheck {
        installed: bool,
        integrated: bool,
        detail: String,
    },
    VlcDownload(DownloadEvent),
    EngineReady(Result<Arc<Engine>, String>),
    ExternalPlayer(Result<(), String>),
}

/// What to do once a stream address has been resolved.
#[derive(Clone)]
pub enum Action {
    PlayExternal,
    PlayVlcExternal,
    /// Open the built-in libVLC player at this position of the current playlist.
    OpenBuiltin { index: usize },
    Copy,
    /// Ask for a save path, then download; the item carries what the file name needs.
    Download { item: LiveItem },
}
