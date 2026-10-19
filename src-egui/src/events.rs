use std::sync::Arc;

use crate::models::LiveItem;
use crate::vlc::Engine;

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
    StreamUrl {
        result: Result<String, String>,
        action: Box<Action>,
    },
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
    Download { live_id: String, title: String },
}
