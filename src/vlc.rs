//! libVLC, loaded at runtime through its memory video-output callbacks, so decoded
//! frames can be uploaded to an egui texture instead of a native child window.

use std::ffi::{c_int, c_uint, c_void, CString};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use libloading::Library;

type FnNew = unsafe extern "C" fn(c_int, *mut *const c_void) -> *mut c_void;
type FnRelease = unsafe extern "C" fn(*mut c_void);
type FnMediaNewLocation = unsafe extern "C" fn(*mut c_void, *const i8) -> *mut c_void;
type FnMediaAddOption = unsafe extern "C" fn(*mut c_void, *const i8);
type FnMediaRelease = unsafe extern "C" fn(*mut c_void);
type FnPlayerNew = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type FnPlayerSetMedia = unsafe extern "C" fn(*mut c_void, *mut c_void);
type FnPlayerPlay = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnPlayerStop = unsafe extern "C" fn(*mut c_void);
type FnPlayerRelease = unsafe extern "C" fn(*mut c_void);
type FnPlayerSetPause = unsafe extern "C" fn(*mut c_void, c_int);
type FnPlayerGetTime = unsafe extern "C" fn(*mut c_void) -> i64;
type FnPlayerSetTime = unsafe extern "C" fn(*mut c_void, i64) -> c_int;
type FnPlayerGetLength = unsafe extern "C" fn(*mut c_void) -> i64;
type FnAudioSetMute = unsafe extern "C" fn(*mut c_void, c_int);
type FnPlayerGetState = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnVideoSetCallbacks = unsafe extern "C" fn(
    *mut c_void,
    Option<unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> *mut c_void>,
    Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const *mut c_void)>,
    Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    *mut c_void,
);
type FnVideoSetFormatCallbacks = unsafe extern "C" fn(
    *mut c_void,
    Option<
        unsafe extern "C" fn(
            *mut *mut c_void,
            *mut i8,
            *mut c_uint,
            *mut c_uint,
            *mut c_uint,
            *mut c_uint,
        ) -> c_uint,
    >,
    Option<unsafe extern "C" fn(*mut c_void)>,
);

// ------------------------------------------------------------- frame buffers

/// Two stable heap buffers that libVLC writes decoded frames into.
pub struct FrameState {
    /// Requested ceiling for the converter output, packed as (w << 32) | h.
    want: AtomicU64,
    /// (width, height, pitch) negotiated with libVLC.
    dims: Mutex<Option<(usize, usize, usize)>>,
    buffers: [AtomicPtr<u8>; 2],
    bytes: AtomicUsize,
    busy: [AtomicBool; 2],
    pick: AtomicUsize,
    displayed: AtomicUsize,
    seq: AtomicU64,
}

unsafe impl Send for FrameState {}
unsafe impl Sync for FrameState {}

impl FrameState {
    fn new(max_w: u32, max_h: u32) -> Arc<Self> {
        Arc::new(FrameState {
            want: AtomicU64::new(((max_w as u64) << 32) | max_h as u64),
            dims: Mutex::new(None),
            buffers: [AtomicPtr::new(ptr::null_mut()), AtomicPtr::new(ptr::null_mut())],
            bytes: AtomicUsize::new(0),
            busy: [AtomicBool::new(false), AtomicBool::new(false)],
            pick: AtomicUsize::new(0),
            displayed: AtomicUsize::new(0),
            seq: AtomicU64::new(0),
        })
    }

    fn free_buffers(&self) {
        for slot in &self.buffers {
            let prev = slot.swap(ptr::null_mut(), Ordering::AcqRel);
            if !prev.is_null() {
                unsafe {
                    let len = self.bytes.load(Ordering::Relaxed);
                    drop(Vec::from_raw_parts(prev, len, len));
                }
            }
        }
        self.bytes.store(0, Ordering::Relaxed);
    }

    /// Called from the format callback: (re)allocate the two picture buffers.
    fn configure(&self, w: usize, h: usize, pitch: usize) {
        let len = pitch * h;
        {
            let mut dims = self.dims.lock().unwrap_or_else(|p| p.into_inner());
            *dims = Some((w, h, pitch));
        }
        if self.bytes.load(Ordering::Acquire) == len
            && !self.buffers[0].load(Ordering::Acquire).is_null()
        {
            return;
        }
        self.free_buffers();
        self.bytes.store(len, Ordering::Release);
        for slot in &self.buffers {
            let mut buf = vec![0u8; len];
            let raw = buf.as_mut_ptr();
            std::mem::forget(buf);
            slot.store(raw, Ordering::Release);
        }
    }

    pub fn size(&self) -> Option<(usize, usize)> {
        self.dims
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .map(|(w, h, _)| (w, h))
    }

    pub fn sequence(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// Copies the newest presented frame into `dst`, swizzled from BGRA to RGBA.
    /// Returns false when that buffer is being written right now.
    pub fn take_rgba(&self, dst: &mut Vec<u8>) -> bool {
        let dims = match *self.dims.lock().unwrap_or_else(|p| p.into_inner()) {
            Some(d) => d,
            None => return false,
        };
        let idx = self.displayed.load(Ordering::Acquire) & 1;
        if self.busy[idx].load(Ordering::Acquire) {
            return false;
        }
        let src = self.buffers[idx].load(Ordering::Acquire);
        if src.is_null() {
            return false;
        }
        let (_w, h, pitch) = dims;
        let len = pitch * h;
        if dst.len() != len {
            dst.resize(len, 0);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), len);
        }
        // RV32 reaches us as B,G,R,x while egui wants R,G,B,A. Swapping whole u32
        // words vectorises; the per-byte loop cost ~8ms for a 544x962 frame.
        for word in bytemuck::cast_slice_mut::<u8, u32>(dst) {
            let s = *word;
            *word = ((s >> 16) & 0xFF) | (s & 0xFF00) | ((s & 0xFF) << 16) | 0xFF00_0000;
        }
        true
    }
}

impl Drop for FrameState {
    fn drop(&mut self) {
        self.free_buffers();
    }
}

// ---------------------------------------------------------------- callbacks

unsafe extern "C" fn cb_lock(opaque: *mut c_void, planes: *mut *mut c_void) -> *mut c_void {
    let st = unsafe { &*(opaque as *const FrameState) };
    let first = st.pick.fetch_add(1, Ordering::Relaxed) & 1;
    for idx in [first, 1 - first] {
        if st.busy[idx]
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            let ptr = st.buffers[idx].load(Ordering::Acquire);
            if ptr.is_null() {
                st.busy[idx].store(false, Ordering::Release);
                return ptr::null_mut();
            }
            unsafe { *planes = ptr as *mut c_void };
            // The index travels as a tagged integer: libVLC hands the same value to
            // unlock() and then to display(), so nothing may be freed in between.
            return (idx + 1) as *mut c_void;
        }
    }
    ptr::null_mut()
}

unsafe extern "C" fn cb_unlock(opaque: *mut c_void, picture: *mut c_void, _planes: *const *mut c_void) {
    let st = unsafe { &*(opaque as *const FrameState) };
    if picture.is_null() {
        return;
    }
    let idx = (picture as usize) - 1;
    st.busy[idx & 1].store(false, Ordering::Release);
}

unsafe extern "C" fn cb_display(opaque: *mut c_void, picture: *mut c_void) {
    let st = unsafe { &*(opaque as *const FrameState) };
    if picture.is_null() {
        return;
    }
    let idx = (picture as usize) - 1;
    st.displayed.store(idx & 1, Ordering::Release);
    st.seq.fetch_add(1, Ordering::Release);
}

/// Negotiates the picture format. Writing smaller width/height makes libVLC
/// rescale on its own converter, so we never pay for a per-frame CPU resize.
unsafe extern "C" fn cb_format(
    opaque: *mut *mut c_void,
    chroma: *mut i8,
    width: *mut c_uint,
    height: *mut c_uint,
    pitches: *mut c_uint,
    lines: *mut c_uint,
) -> c_uint {
    let raw = unsafe { *opaque };
    if raw.is_null() || chroma.is_null() || width.is_null() || height.is_null() {
        return 0;
    }
    let st = unsafe { &*(raw as *const FrameState) };
    let src_w = unsafe { *width } as usize;
    let src_h = unsafe { *height } as usize;
    if src_w == 0 || src_h == 0 {
        return 0;
    }

    let (max_w, max_h) = {
        let v = st.want.load(Ordering::Relaxed);
        ((v >> 32) as usize, v as usize)
    };
    let scale = (max_w as f64 / src_w as f64)
        .min(max_h as f64 / src_h as f64)
        .min(1.0);
    let out_w = ((src_w as f64 * scale).round() as usize) & !1;
    let out_h = ((src_h as f64 * scale).round() as usize) & !1;
    let out_w = out_w.max(16);
    let out_h = out_h.max(16);
    let pitch = out_w * 4;

    for (i, b) in b"RV32".iter().enumerate() {
        unsafe { *chroma.add(i) = *b as i8 };
    }
    unsafe {
        *width = out_w as c_uint;
        *height = out_h as c_uint;
        *pitches = pitch as c_uint;
        *lines = out_h as c_uint;
    }
    st.configure(out_w, out_h, pitch);
    2
}

// ------------------------------------------------------------------- loading

fn candidate_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("vlc"));
            v.push(dir.to_path_buf());
        }
    }
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vlc"));
    // Kept so a working tree that still has the old Tauri layout works unchanged.
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src-tauri/vlc"));
    v.push(crate::config::dir().join("vlc"));
    v.push(PathBuf::from(r"C:\Program Files\VideoLAN\VLC"));
    v.push(PathBuf::from(r"C:\Program Files (x86)\VideoLAN\VLC"));
    v
}

fn find_libvlc() -> Option<(PathBuf, PathBuf)> {
    for dir in candidate_dirs() {
        for name in ["libvlc.dll", "libvlc.dylib", "libvlc.so.5", "libvlc.so"] {
            let dll = dir.join(name);
            let plugins = dir.join("plugins");
            if dll.exists() && plugins.is_dir() {
                return Some((dll, plugins));
            }
        }
    }
    None
}

/// Absolute path without the `\?\` prefix or `..` hops, for showing in the UI.
fn display_path(path: &std::path::Path) -> String {
    path.canonicalize()
        .map(|p| {
            let s = p.display().to_string();
            s.strip_prefix("\\\\?\\").map(|s| s.to_owned()).unwrap_or(s)
        })
        .unwrap_or_else(|_| path.display().to_string())
}

pub struct Status {
    pub bundled: bool,
    pub loadable: bool,
    pub detail: String,
}

pub fn status() -> Status {
    match find_libvlc() {
        None => Status {
            bundled: false,
            loadable: false,
            detail: "未找到 libvlc 动态库".to_owned(),
        },
        Some((dll, _)) => {
            let bundled = dll.starts_with(&crate::config::dir());
            let loadable = Engine::load().is_ok();
            Status {
                bundled,
                loadable,
                detail: display_path(&dll),
            }
        }
    }
}

/// Fetches the official libVLC runtime from NuGet into the app data directory so a
/// machine without VLC installed can still play RTMP.
pub fn spawn_download(req: crate::app::Requester) {
    std::thread::spawn(move || {
        let send = |kind| req.send(crate::events::Event::VlcDownload(kind));
        send(crate::events::DownloadEvent::Progress(0.0));

        let result = (|| -> Result<(), String> {
            const URL: &str = "https://www.nuget.org/api/v2/package/VideoLAN.LibVLC.Windows/3.0.21";
            let target = crate::config::dir().join("vlc");
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .map_err(|e| e.to_string())?;

            send(crate::events::DownloadEvent::Progress(4.0));
            let mut resp = client.get(URL).send().map_err(|e| format!("下载失败: {e}"))?;
            let total = resp.content_length().unwrap_or(0);
            let mut got = 0u64;
            let mut bytes: Vec<u8> = Vec::with_capacity(total as usize);
            let mut buf = [0u8; 65536];
            loop {
                let n = std::io::Read::read(&mut resp, &mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                got += n as u64;
                if total > 0 {
                    send(crate::events::DownloadEvent::Progress(
                        5.0 + (got as f64 / total as f64) * 75.0,
                    ));
                }
            }

            send(crate::events::DownloadEvent::Progress(82.0));
            let mut archive =
                zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
            let prefix = (0..archive.len())
                .find_map(|i| {
                    let entry = archive.by_index(i).ok()?;
                    let name = entry.name().replace('\\', "/");
                    if name.ends_with("/libvlc.dll") || name.ends_with("/libvlccore.dll") {
                        name.rfind('/').map(|idx| name[..=idx].to_string())
                    } else {
                        None
                    }
                })
                .ok_or("包内未找到 libvlc.dll")?;

            if target.exists() {
                let _ = std::fs::remove_dir_all(&target);
            }
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;

            let count = archive.len().max(1);
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
                let name = entry.name().replace('\\', "/");
                if !name.starts_with(&prefix) {
                    continue;
                }
                let out = target.join(&name[prefix.len()..]);
                if entry.is_dir() {
                    let _ = std::fs::create_dir_all(&out);
                } else {
                    if let Some(parent) = out.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                    std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
                }
                if i % 24 == 0 {
                    send(crate::events::DownloadEvent::Progress(
                        83.0 + (i as f64 / count as f64) * 15.0,
                    ));
                }
            }
            Ok(())
        })();

        match result {
            Ok(()) => send(crate::events::DownloadEvent::Success),
            Err(e) => send(crate::events::DownloadEvent::Error(e)),
        }
    });
}

// ------------------------------------------------------------------- engine

struct Ffi {
    _lib: Library,
    new: FnNew,
    release: FnRelease,
    media_new_location: FnMediaNewLocation,
    media_add_option: FnMediaAddOption,
    media_release: FnMediaRelease,
    player_new: FnPlayerNew,
    player_set_media: FnPlayerSetMedia,
    player_play: FnPlayerPlay,
    player_stop: FnPlayerStop,
    player_release: FnPlayerRelease,
    player_set_pause: FnPlayerSetPause,
    player_get_time: FnPlayerGetTime,
    player_set_time: FnPlayerSetTime,
    player_get_length: FnPlayerGetLength,
    audio_set_mute: FnAudioSetMute,
    player_get_state: FnPlayerGetState,
    video_set_callbacks: FnVideoSetCallbacks,
    video_set_format_callbacks: FnVideoSetFormatCallbacks,
}

unsafe impl Send for Ffi {}
unsafe impl Sync for Ffi {}

pub struct Engine {
    ffi: Arc<Ffi>,
    instance: *mut c_void,
}

unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}

impl Engine {
    pub fn load() -> Result<Engine, String> {
        let (dll, plugins) = find_libvlc().ok_or_else(|| "未找到 libvlc 动态库".to_owned())?;
        let dir = dll.parent().map(|p| p.to_path_buf()).unwrap_or_default();

        unsafe {
            if let Some(dirstr) = dir.to_str() {
                match std::env::var("PATH") {
                    Ok(path) if !path.split(';').any(|p| p.eq_ignore_ascii_case(dirstr)) => {
                        std::env::set_var("PATH", format!("{dirstr};{path}"));
                    }
                    Ok(_) => {}
                    Err(_) => std::env::set_var("PATH", dirstr),
                }
            }
            std::env::set_var("VLC_PLUGIN_PATH", &plugins);
        }

        let lib =
            unsafe { Library::new(&dll) }.map_err(|e| format!("加载 {} 失败: {e}", dll.display()))?;

macro_rules! bind {
            ($field:ident, $name:expr, $ty:ty) => {
                let $field: $ty = unsafe {
                    *lib.get($name.as_bytes())
                        .map_err(|e| format!("缺少符号 {}: {e}", $name))?
                };
            };
        }

        bind!(new, "libvlc_new", FnNew);
        bind!(release, "libvlc_release", FnRelease);
        bind!(media_new_location, "libvlc_media_new_location", FnMediaNewLocation);
        bind!(media_add_option, "libvlc_media_add_option", FnMediaAddOption);
        bind!(media_release, "libvlc_media_release", FnMediaRelease);
        bind!(player_new, "libvlc_media_player_new", FnPlayerNew);
        bind!(player_set_media, "libvlc_media_player_set_media", FnPlayerSetMedia);
        bind!(player_play, "libvlc_media_player_play", FnPlayerPlay);
        bind!(player_stop, "libvlc_media_player_stop", FnPlayerStop);
        bind!(player_release, "libvlc_media_player_release", FnPlayerRelease);
        bind!(player_set_pause, "libvlc_media_player_set_pause", FnPlayerSetPause);
        bind!(player_get_time, "libvlc_media_player_get_time", FnPlayerGetTime);
        bind!(player_set_time, "libvlc_media_player_set_time", FnPlayerSetTime);
        bind!(player_get_length, "libvlc_media_player_get_length", FnPlayerGetLength);
        bind!(player_get_state, "libvlc_media_player_get_state", FnPlayerGetState);
        bind!(audio_set_mute, "libvlc_audio_set_mute", FnAudioSetMute);
        bind!(video_set_callbacks, "libvlc_video_set_callbacks", FnVideoSetCallbacks);
        bind!(video_set_format_callbacks, "libvlc_video_set_format_callbacks", FnVideoSetFormatCallbacks);

        let ffi = Arc::new(Ffi {
            _lib: lib,
            new,
            release,
            media_new_location,
            media_add_option,
            media_release,
            player_new,
            player_set_media,
            player_play,
            player_stop,
            player_release,
            player_set_pause,
            player_get_time,
            player_set_time,
            player_get_length,
            player_get_state,
            audio_set_mute,
            video_set_callbacks,
            video_set_format_callbacks,
        });

        let plugin_path = plugins.display().to_string();
        let loud = std::env::var("LIVE48_VLC_VERBOSE").is_ok();
        let mut args: Vec<CString> = Vec::new();
        args.push(CString::new("live48").unwrap());
        if loud {
            args.push(CString::new("--verbose=2").unwrap());
        } else {
            args.push(CString::new("--quiet").unwrap());
            args.push(CString::new("--verbose=0").unwrap());
        }
        args.extend([
            CString::new("--network-caching=300").unwrap(),
            CString::new("--live-caching=300").unwrap(),
            CString::new("--file-caching=400").unwrap(),
            CString::new("--no-video-title-show").unwrap(),
            CString::new("--no-stats").unwrap(),
            // Subtitles/OSD would be blended into our RGBA buffer by the spu mixer;
            // a dummy text renderer keeps the picture clean and cheap.
            CString::new("--text-renderer=dummy").unwrap(),
            CString::new("--no-osd").unwrap(),
        ]);
        let mut argv: Vec<*const i8> = args.iter().map(|s| s.as_ptr()).collect();
        let instance =
            unsafe { (ffi.new)(argv.len() as c_int, argv.as_mut_ptr() as *mut *const c_void) };
        if instance.is_null() {
            return Err(format!(
                "libvlc_new 失败 (plugin-path={plugin_path})，请检查 VLC 引擎是否完整"
            ));
        }

        Ok(Engine {
            ffi,
            instance,
        })
    }

    /// Starts playing `url` into memory buffers. RTMP, HLS and HTTP-TS all go
    /// through libVLC's own demuxers, so no transcode round-trip is needed.
    pub fn open(&self, url: &str, max_w: u32, max_h: u32) -> Result<Session, String> {
        let media = self.new_media(url)?;
        if url.starts_with("http") {
            self.add_option(media, ":http-user-agent=SNH48 ENGINE")?;
        }
        let mp = unsafe { (self.ffi.player_new)(self.instance) };
        if mp.is_null() {
            unsafe { (self.ffi.media_release)(media) };
            return Err("无法创建播放器对象".to_string());
        }

        let state = FrameState::new(max_w, max_h);
        let vlc_ref: *mut FrameState = Arc::into_raw(state.clone()) as *mut FrameState;

        unsafe {
            (self.ffi.video_set_callbacks)(
                mp,
                Some(cb_lock),
                Some(cb_unlock),
                Some(cb_display),
                vlc_ref as *mut c_void,
            );
            (self.ffi.video_set_format_callbacks)(mp, Some(cb_format), None);
            (self.ffi.player_set_media)(mp, media);
        }

        if unsafe { (self.ffi.player_play)(mp) } != 0 {
            unsafe {
                (self.ffi.player_release)(mp);
                (self.ffi.media_release)(media);
                drop(Arc::from_raw(vlc_ref));
            }
            return Err("VLC 播放启动失败".to_string());
        }

        Ok(Session {
            ffi: self.ffi.clone(),
            mp,
            media,
            state,
            vlc_ref,
        })
    }

    fn new_media(&self, url: &str) -> Result<*mut c_void, String> {
        let c_url = CString::new(url).map_err(|_| "URL 含空字节".to_string())?;
        let media = unsafe { (self.ffi.media_new_location)(self.instance, c_url.as_ptr()) };
        if media.is_null() {
            return Err("无法创建媒体对象".to_string());
        }
        Ok(media)
    }

    fn add_option(&self, media: *mut c_void, opt: &str) -> Result<(), String> {
        let c = CString::new(opt).map_err(|_| "选项含空字节".to_string())?;
        unsafe { (self.ffi.media_add_option)(media, c.as_ptr()) };
        Ok(())
    }

    /// Copies `url` into `out` on its own connection, decoding nothing: the stream
    /// output chain taps the demuxed elementary streams, and with no video or audio
    /// output there is no window and almost no CPU cost. Recording therefore never
    /// disturbs what is on screen.
    pub fn record(&self, url: &str, out: &str) -> Result<Recorder, String> {
        let media = self.new_media(url)?;
        if url.starts_with("http") {
            self.add_option(media, ":http-user-agent=SNH48 ENGINE")?;
        }
        // VLC's MRL parser takes Windows separators out of the picture, and the
        // quoting keeps the drive colon from being read as an option separator.
        let dst = out.replace('\\', "/");
        self.add_option(media, &format!(":sout=#std{{access=file,mux=ts,dst=\"{dst}\"}}"))?;
        for opt in [":no-video", ":no-audio", ":network-caching=1000"] {
            self.add_option(media, opt)?;
        }
        let mp = unsafe { (self.ffi.player_new)(self.instance) };
        if mp.is_null() {
            unsafe { (self.ffi.media_release)(media) };
            return Err("无法创建录制对象".to_string());
        }
        unsafe { (self.ffi.player_set_media)(mp, media) };
        if unsafe { (self.ffi.player_play)(mp) } != 0 {
            unsafe {
                (self.ffi.player_release)(mp);
                (self.ffi.media_release)(media);
            }
            return Err("VLC 录制启动失败".to_string());
        }
        Ok(Recorder {
            ffi: self.ffi.clone(),
            mp,
            media,
            out: out.to_owned(),
        })
    }
}

/// Writes one stream to one file until it ends or is dropped.
pub struct Recorder {
    ffi: Arc<Ffi>,
    mp: *mut c_void,
    media: *mut c_void,
    pub out: String,
}

unsafe impl Send for Recorder {}

impl Recorder {
    /// True while VLC is still pulling the stream; once the source dies the state
    /// settles on stopped/ended/error and the caller decides whether to resume.
    pub fn running(&self) -> bool {
        matches!(unsafe { (self.ffi.player_get_state)(self.mp) }, 1 | 2 | 3 | 8)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        unsafe {
            (self.ffi.player_stop)(self.mp);
            (self.ffi.player_release)(self.mp);
            (self.ffi.media_release)(self.media);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { (self.ffi.release)(self.instance) };
    }
}

pub struct Session {
    ffi: Arc<Ffi>,
    mp: *mut c_void,
    media: *mut c_void,
    state: Arc<FrameState>,
    vlc_ref: *mut FrameState,
}

unsafe impl Send for Session {}

impl Session {
    pub fn frames(&self) -> &Arc<FrameState> {
        &self.state
    }

    pub fn set_paused(&self, paused: bool) {
        unsafe { (self.ffi.player_set_pause)(self.mp, paused as c_int) };
    }

    pub fn time_ms(&self) -> i64 {
        unsafe { (self.ffi.player_get_time)(self.mp) }
    }

    pub fn set_time_ms(&self, ms: i64) {
        unsafe {
            (self.ffi.player_set_time)(self.mp, ms);
        }
    }

    pub fn length_ms(&self) -> i64 {
        unsafe { (self.ffi.player_get_length)(self.mp) }
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe { (self.ffi.audio_set_mute)(self.mp, muted as c_int) };
    }

    pub fn muted(&self) -> bool {
        false
    }

    /// libVLC state: 1 opening, 2 buffering, 3 playing, 4 paused, 5 stopped,
    /// 6 ended, 7 error, 8 connecting.
    pub fn vlc_state(&self) -> i32 {
        unsafe { (self.ffi.player_get_state)(self.mp) }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            // stop() blocks until the vout has released every callback reference,
            // so dropping the extra Arc afterwards is safe.
            (self.ffi.player_stop)(self.mp);
            (self.ffi.player_release)(self.mp);
            (self.ffi.media_release)(self.media);
            drop(Arc::from_raw(self.vlc_ref));
        }
    }
}
