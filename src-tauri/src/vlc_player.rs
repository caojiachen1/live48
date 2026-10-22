use std::ffi::{c_int, c_void, CString};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use libloading::{Library, Symbol};

// ============================================================
// libVLC FFI 类型别名
// ============================================================

type LibvlcNew = unsafe extern "C" fn(c_int, *mut *const c_void) -> *mut c_void;
type LibvlcRelease = unsafe extern "C" fn(*mut c_void);
type LibvlcMediaNewLocation = unsafe extern "C" fn(*mut c_void, *const i8) -> *mut c_void;
type LibvlcMediaRelease = unsafe extern "C" fn(*mut c_void);
type LibvlcMediaPlayerNew = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type LibvlcMediaPlayerSetMedia = unsafe extern "C" fn(*mut c_void, *mut c_void);
type LibvlcMediaPlayerSetHwnd = unsafe extern "C" fn(*mut c_void, *mut c_void);
#[allow(dead_code)]
type LibvlcMediaPlayerSetNsobject = unsafe extern "C" fn(*mut c_void, *mut c_void);
#[allow(dead_code)]
type LibvlcMediaPlayerSetXwindow = unsafe extern "C" fn(*mut c_void, u32);
type LibvlcMediaPlayerPlay = unsafe extern "C" fn(*mut c_void) -> c_int;
type LibvlcMediaPlayerStop = unsafe extern "C" fn(*mut c_void);
#[allow(dead_code)]
type LibvlcMediaPlayerIsPlaying = unsafe extern "C" fn(*mut c_void) -> c_int;
type LibvlcMediaPlayerRelease = unsafe extern "C" fn(*mut c_void);
type LibvlcMediaAddOption = unsafe extern "C" fn(*mut c_void, *const i8);

/// 动态加载的 libVLC 函数表（裸函数指针，无生命周期问题）
pub struct Libvlc {
    _lib: Library,
    new: unsafe extern "C" fn(c_int, *mut *const c_void) -> *mut c_void,
    release: unsafe extern "C" fn(*mut c_void),
    media_new_location: unsafe extern "C" fn(*mut c_void, *const i8) -> *mut c_void,
    media_release: unsafe extern "C" fn(*mut c_void),
    player_new: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    player_set_media: unsafe extern "C" fn(*mut c_void, *mut c_void),
    player_set_hwnd: unsafe extern "C" fn(*mut c_void, *mut c_void),
    #[allow(dead_code)]
    player_set_nsobject: unsafe extern "C" fn(*mut c_void, *mut c_void),
    #[allow(dead_code)]
    player_set_xwindow: unsafe extern "C" fn(*mut c_void, u32),
    player_play: unsafe extern "C" fn(*mut c_void) -> c_int,
    player_stop: unsafe extern "C" fn(*mut c_void),
    #[allow(dead_code)]
    player_is_playing: unsafe extern "C" fn(*mut c_void) -> c_int,
    player_release: unsafe extern "C" fn(*mut c_void),
    media_add_option: unsafe extern "C" fn(*mut c_void, *const i8),
}

/// 获取 VLC DLL 自带的目录（公开给 lib.rs 使用）
pub fn bundled_dir() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        // 开发模式：相对于 crate 源目录
        return Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vlc"));
    }

    #[cfg(not(debug_assertions))]
    {
        // 生产模式：相对于可执行文件
        return std::env::current_exe().ok()?.parent().map(|p| p.join("vlc"));
    }
}

/// 从 NuGet 下载 VLC DLL 到目标目录（仅 Windows）
#[cfg(target_os = "windows")]
fn download_vlc_to(target_dir: &std::path::Path) -> Result<(), String> {
    const URL: &str =
        "https://www.nuget.org/api/v2/package/VideoLAN.LibVLC.Windows/3.0.21";

    eprintln!("[vlc] 下载 VLC DLL 中 (从 NuGet) ...");

    let resp = reqwest::blocking::get(URL)
        .map_err(|e| format!("下载 VLC DLL 失败: {}", e))?;
    let bytes = resp.bytes().map_err(|e| format!("读取响应失败: {}", e))?;

    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("解压 zip 失败: {}", e))?;

    // 找到 libvlc.dll 所在的前缀目录（如 "build/x64/"）
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
        .ok_or("在 NuGet 包中未找到 libvlc.dll")?;

    eprintln!("[vlc] 提取到 {} ...", target_dir.display());

    std::fs::create_dir_all(target_dir).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().replace('\\', "/");

        if !name.starts_with(&prefix) {
            continue;
        }

        let rel_path = &name[prefix.len()..];
        let target_path = target_dir.join(rel_path);
        let target_str = target_path.to_string_lossy().to_string();

        if entry.is_dir() {
            let _ = std::fs::create_dir_all(&target_path);
        } else if entry.is_file() {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&target_path)
                .map_err(|e| format!("创建 {} 失败: {}", target_str, e))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| format!("写入 {} 失败: {}", target_str, e))?;
        }
    }

    eprintln!("[vlc] 下载完成");
    Ok(())
}

/// 检查 VLC 完整性：验证所有必需文件存在且可以加载 libvlc
pub fn check_vlc_integrity() -> Result<(), String> {
    let Some(target) = bundled_dir() else {
        return Err("无法确定 VLC 目录位置".to_string());
    };

    // 检查关键文件
    let libvlc_dll = target.join("libvlc.dll");
    let libvlccore_dll = target.join("libvlccore.dll");
    let plugins_dir = target.join("plugins");

    if !libvlc_dll.exists() {
        return Err("libvlc.dll 不存在".to_string());
    }
    if !libvlccore_dll.exists() {
        return Err("libvlccore.dll 不存在".to_string());
    }
    if !plugins_dir.exists() || !plugins_dir.is_dir() {
        return Err("plugins 目录不存在".to_string());
    }

    // 尝试加载 libvlc 验证 DLL 可正常加载
    match Libvlc::load() {
        Ok(_) => {
            eprintln!("[vlc] 完整性检查通过");
            Ok(())
        }
        Err(e) => {
            Err(format!("libvlc 加载验证失败: {}", e))
        }
    }
}

/// 确保自带 VLC DLL 存在（如缺失则尝试自动下载）
/// 应在 Tauri 启动阶段调用，避免在持有 Mutex 时触发下载
pub fn ensure_vlc_downloaded() {
    // 不再自动下载，改为由用户在设置面板手动触发
}

/// 查找自带的（bundled）libvlc 动态库并验证 plugins/ 目录存在
fn find_bundled_libvlc() -> Option<PathBuf> {
    /// 检查路径有效（DLL 存在 + plugins/ 目录存在）
    fn validate_libvlc_path(lib_path: &std::path::Path) -> bool {
        if !lib_path.exists() {
            return false;
        }
        // 必须 plugins/ 目录也存在于同一父目录
        if let Some(parent) = lib_path.parent() {
            let plugins = parent.join("plugins");
            if plugins.exists() && plugins.is_dir() {
                return true;
            }
        }
        false
    }

    // Debug 模式：优先用 crate 源目录下的 vlc/（下载目标位置，plugins 完整）
    #[cfg(debug_assertions)]
    {
        let vlc_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vlc");
        #[cfg(target_os = "windows")]
        {
            let p = vlc_dir.join("libvlc.dll");
            if validate_libvlc_path(&p) {
                return Some(p);
            }
        }
        #[cfg(target_os = "macos")]
        {
            let p = vlc_dir.join("libvlc.dylib");
            if validate_libvlc_path(&p) {
                return Some(p);
            }
        }
        #[cfg(target_os = "linux")]
        {
            let p = vlc_dir.join("libvlc.so");
            if validate_libvlc_path(&p) {
                return Some(p);
            }
        }
    }

    // 相对于当前可执行文件（生产 bundle）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            #[cfg(target_os = "windows")]
            {
                let candidates = [
                    dir.join("libvlc.dll"),
                    dir.join("vlc").join("libvlc.dll"),
                ];
                for p in candidates {
                    if validate_libvlc_path(&p) {
                        return Some(p);
                    }
                }
            }
            #[cfg(target_os = "macos")]
            {
                // Tauri .app bundle: Resources/vlc/
                let candidates = [
                    dir.join("../Resources/vlc/libvlc.dylib"),
                    dir.join("libvlc.dylib"),
                    dir.join("vlc").join("libvlc.dylib"),
                ];
                for p in candidates {
                    if validate_libvlc_path(&p) {
                        return Some(p);
                    }
                }
            }
            #[cfg(target_os = "linux")]
            {
                let candidates = [
                    dir.join("libvlc.so"),
                    dir.join("vlc").join("libvlc.so"),
                ];
                for p in candidates {
                    if validate_libvlc_path(&p) {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}

/// 查找系统中已安装的 libvlc 共享库（含自带路径优先）
fn find_libvlc() -> Option<PathBuf> {
    // 0. 自带路径优先
    if let Some(p) = find_bundled_libvlc() {
        return Some(p);
    }

    // 1. 系统安装路径
    #[cfg(target_os = "windows")]
    {
        let candidates = vec![
            PathBuf::from(r"C:\Program Files\VideoLAN\VLC\libvlc.dll"),
            PathBuf::from(r"C:\Program Files (x86)\VideoLAN\VLC\libvlc.dll"),
        ];
        for p in &candidates {
            if p.exists() {
                return Some(p.clone());
            }
        }
        // 让 libloading 在 PATH 和系统目录中搜索
        return Some(PathBuf::from("libvlc.dll"));
    }

    #[cfg(target_os = "macos")]
    {
        let candidates = vec![
            PathBuf::from("/Applications/VLC.app/Contents/MacOS/lib/libvlc.dylib"),
            PathBuf::from("/usr/local/lib/libvlc.dylib"),
            PathBuf::from("/opt/homebrew/lib/libvlc.dylib"),
        ];
        for p in &candidates {
            if p.exists() { return Some(p); }
        }
        return Some(PathBuf::from("libvlc.dylib"));
    }

    #[cfg(target_os = "linux")]
    {
        let extra = vec![
            PathBuf::from("/usr/lib/x86_64-linux-gnu/libvlc.so"),
            PathBuf::from("/usr/lib/libvlc.so"),
            PathBuf::from("/usr/local/lib/libvlc.so"),
        ];
        for p in &extra {
            if p.exists() { return Some(p); }
        }
        return Some(PathBuf::from("libvlc.so"));
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    { None }
}

impl Libvlc {
    /// 动态加载 libVLC 并绑定函数指针
    pub fn load() -> Result<Self, String> {
        let lib_path = find_libvlc().ok_or_else(|| "找不到 libvlc 动态库".to_string())?;
        eprintln!("[vlc] load: 找到 libvlc 路径 = {}", lib_path.display());

        // 确保 VLC 目录在 DLL 搜索路径中（Windows 需要找到 libvlccore.dll 和 plugins/）
        #[cfg(target_os = "windows")]
        {
            if let Some(parent) = lib_path.parent() {
                eprintln!("[vlc] load: libvlc 父目录 = {}", parent.display());
                if let Ok(var) = std::env::var("PATH") {
                    let new_path = format!("{};{}", parent.display(), var);
                    std::env::set_var("PATH", &new_path);
                    eprintln!("[vlc] load: 已将 {} 加入 PATH", parent.display());
                } else {
                    std::env::set_var("PATH", &parent.display().to_string());
                    eprintln!("[vlc] load: PATH 未设置，已设置为 {}", parent.display());
                }
                let plugin_path = parent.join("plugins");
                if plugin_path.exists() {
                    std::env::set_var("VLC_PLUGIN_PATH", plugin_path.display().to_string());
                    eprintln!("[vlc] load: VLC_PLUGIN_PATH = {}", plugin_path.display());
                } else {
                    eprintln!("[vlc] load: 警告: plugins 目录不存在: {}", plugin_path.display());
                }
            }
        }

        let lib = unsafe {
            Library::new(&lib_path).map_err(|e| {
                let msg = format!("加载 libvlc 失败 (路径: {}): {}", lib_path.display(), e);
                eprintln!("[vlc] {}", msg);
                msg
            })?
        };
        eprintln!("[vlc] load: Library::new 成功");

        macro_rules! get_fn {
            ($lib:expr, $name:expr, $ty:ty) => {
                unsafe {
                    let sym: Symbol<'_, $ty> = $lib
                        .get($name.as_bytes())
                        .map_err(|e| format!("找不到符号 {}: {}", $name, e))?;
                    *sym
                }
            };
        }

        Ok(Libvlc {
            new: get_fn!(lib, "libvlc_new", LibvlcNew),
            release: get_fn!(lib, "libvlc_release", LibvlcRelease),
            media_new_location: get_fn!(lib, "libvlc_media_new_location", LibvlcMediaNewLocation),
            media_release: get_fn!(lib, "libvlc_media_release", LibvlcMediaRelease),
            player_new: get_fn!(lib, "libvlc_media_player_new", LibvlcMediaPlayerNew),
            player_set_media: get_fn!(lib, "libvlc_media_player_set_media", LibvlcMediaPlayerSetMedia),
            player_set_hwnd: get_fn!(lib, "libvlc_media_player_set_hwnd", LibvlcMediaPlayerSetHwnd),
            player_set_nsobject: get_fn!(lib, "libvlc_media_player_set_nsobject", LibvlcMediaPlayerSetNsobject),
            player_set_xwindow: get_fn!(lib, "libvlc_media_player_set_xwindow", LibvlcMediaPlayerSetXwindow),
            player_play: get_fn!(lib, "libvlc_media_player_play", LibvlcMediaPlayerPlay),
            player_stop: get_fn!(lib, "libvlc_media_player_stop", LibvlcMediaPlayerStop),
            player_is_playing: get_fn!(lib, "libvlc_media_player_is_playing", LibvlcMediaPlayerIsPlaying),
            player_release: get_fn!(lib, "libvlc_media_player_release", LibvlcMediaPlayerRelease),
            media_add_option: get_fn!(lib, "libvlc_media_add_option", LibvlcMediaAddOption),
            _lib: lib,
        })
    }

    pub fn create_instance(&self) -> Result<VlcInstancePtr, String> {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        eprintln!("[vlc] current_dir = {}", current_dir.display());

        let mut vlc_path = current_dir.join("vlc");
        if !vlc_path.exists() {
            eprintln!("[vlc] {}/vlc 不存在，尝试 src-tauri/vlc", current_dir.display());
            vlc_path = current_dir.join("src-tauri/vlc");
        }
        if !vlc_path.exists() {
            eprintln!("[vlc] 警告: vlc 目录不存在: {}", vlc_path.display());
        }
        // 注意: 不使用 canonicalize()，因为 Windows 上它会返回 \\?\ 前缀路径，
        // 而 libvlc 无法正确解析带有 \\?\ 前缀的 --plugin-path 参数
        eprintln!("[vlc] vlc_path = {}", vlc_path.display());
        
        let plugins_path = vlc_path.join("plugins");
        // 使用 dunce 或手动去掉 \\?\ 前缀，确保路径对 libvlc 友好
        let plugins_path_str = {
            let raw = plugins_path.to_str().unwrap_or_default().to_string();
            // 去掉 Windows UNC 前缀 \\?\ 
            if raw.starts_with(r"\\?\") {
                raw[4..].to_string()
            } else {
                raw
            }
        };
        let vlc_path_str = {
            let raw = vlc_path.to_str().unwrap_or_default().to_string();
            if raw.starts_with(r"\\?\") {
                raw[4..].to_string()
            } else {
                raw
            }
        };
        eprintln!("[vlc] plugins_path = {}", plugins_path_str);
        eprintln!("[vlc] plugins 目录存在: {}", plugins_path.exists());

        // 诊断: 列出 plugins 目录下的子目录（帮助定位插件缺失问题）
        if plugins_path.exists() {
            if let Ok(entries) = std::fs::read_dir(&plugins_path) {
                let dirs: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                eprintln!("[vlc] plugins 子目录: {:?}", dirs);
            }
        }

        // 诊断: 检查关键 DLL 是否存在
        let libvlccore = vlc_path.join("libvlccore.dll");
        eprintln!("[vlc] libvlccore.dll 存在: {}", libvlccore.exists());

        unsafe {
            if let Some(path) = std::env::var_os("PATH") {
                let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
                if !paths.contains(&vlc_path) {
                    paths.push(vlc_path.clone());
                    if let Ok(new_path) = std::env::join_paths(paths) {
                        std::env::set_var("PATH", &new_path);
                    }
                }
            }
            std::env::set_var("VLC_PLUGIN_PATH", &plugins_path_str);
        }

        let plugins_arg = format!("--plugin-path={}", plugins_path_str);

        // libvlc_new 的 argc 含 argv[0]（程序名），参照 libvlc 官方文档:
        // "argc is the number of arguments (should be 0 or 1+)"
        // 第一个参数应为程序名（可以是任意字符串）
        let args = [
            CString::new("vlc").unwrap(),
            CString::new("--verbose=0").unwrap(),
            CString::new(plugins_arg).unwrap(),
            // 低延迟选项：最小化 VLC 内部缓冲
            CString::new("--network-caching=100").unwrap(),
            CString::new("--sout-mux-caching=50").unwrap(),
            CString::new("--file-caching=100").unwrap(),
            CString::new("--live-caching=100").unwrap(),
        ];
        let mut c_args: Vec<*const std::ffi::c_char> = args.iter().map(|s| s.as_ptr()).collect();
        let ptr = unsafe { (self.new)(c_args.len() as c_int, c_args.as_mut_ptr() as *mut *const c_void) };
        if ptr.is_null() {
            let err_msg = format!(
                "libvlc_new 失败! vlc_path={}, plugins_path={}, plugins存在={}, libvlccore存在={}",
                vlc_path_str,
                plugins_path_str,
                plugins_path.exists(),
                libvlccore.exists()
            );
            eprintln!("[vlc] {}", err_msg);
            return Err(err_msg);
        }
        eprintln!("[vlc] libvlc_new 成功, instance={:p}", ptr);
        Ok(VlcInstancePtr(ptr))
    }

    pub fn create_media(&self, instance: *mut c_void, url: &str) -> Result<VlcMediaPtr, String> {
        let c_url = CString::new(url).map_err(|_| "URL 含空字节".to_string())?;
        let ptr = unsafe { (self.media_new_location)(instance, c_url.as_ptr()) };
        if ptr.is_null() {
            return Err("libvlc_media_new_location 失败".to_string());
        }
        Ok(VlcMediaPtr(ptr))
    }

    pub fn create_player(&self, instance: *mut c_void) -> Result<VlcPlayerPtr, String> {
        let ptr = unsafe { (self.player_new)(instance) };
        if ptr.is_null() {
            return Err("libvlc_media_player_new 失败".to_string());
        }
        Ok(VlcPlayerPtr(ptr))
    }

    /// 给 media 添加选项（如 sout 转码参数）
    pub fn add_media_option(&self, media: *mut c_void, option: &str) -> Result<(), String> {
        let c_option = CString::new(option).map_err(|_| "option 含空字节".to_string())?;
        unsafe {
            (self.media_add_option)(media, c_option.as_ptr());
        }
        Ok(())
    }
}

// Opaque 指针包装——生命周期由 VlcPlayer 统一管理
pub struct VlcInstancePtr(pub *mut c_void);
pub struct VlcMediaPtr(pub *mut c_void);
pub struct VlcPlayerPtr(pub *mut c_void);

unsafe impl Send for VlcInstancePtr {}
unsafe impl Send for VlcMediaPtr {}
unsafe impl Send for VlcPlayerPtr {}

/// 嵌入式 VLC 播放器（libVLC 动态加载）
pub struct VlcPlayer {
    pub libvlc: Option<Libvlc>,
    pub instance: Option<VlcInstancePtr>,
    media: Option<VlcMediaPtr>,
    player: Option<VlcPlayerPtr>,
}

impl VlcPlayer {
    pub fn new() -> Self {
        VlcPlayer { libvlc: None, instance: None, media: None, player: None }
    }

    /// 初始化 libVLC 引擎（幂等）
    pub fn init(&mut self) -> Result<(), String> {
        if self.instance.is_some() {
            return Ok(());
        }
        let libvlc = Libvlc::load()?;
        let inst = libvlc.create_instance()?;
        self.libvlc = Some(libvlc);
        self.instance = Some(inst);
        Ok(())
    }

    /// 播放流地址，渲染到给定窗口句柄
    pub fn play(
        &mut self,
        url: &str,
        #[cfg(target_os = "windows")] window_handle: *mut c_void,
        #[cfg(target_os = "macos")] window_handle: *mut c_void,
        #[cfg(target_os = "linux")] window_handle: u32,
    ) -> Result<(), String> {
        self.stop_inner();

        let libvlc = self.libvlc.as_ref().ok_or("VLC 未初始化")?;
        let instance = self.instance.as_ref().ok_or("VLC 实例未创建")?;

        let media = libvlc.create_media(instance.0, url)?;
        let player = libvlc.create_player(instance.0)?;

        unsafe { (libvlc.player_set_media)(player.0, media.0); }

        #[cfg(target_os = "windows")]
        unsafe { (libvlc.player_set_hwnd)(player.0, window_handle); }
        #[cfg(target_os = "macos")]
        unsafe { (libvlc.player_set_nsobject)(player.0, window_handle); }
        #[cfg(target_os = "linux")]
        unsafe { (libvlc.player_set_xwindow)(player.0, window_handle); }

        let ret = unsafe { (libvlc.player_play)(player.0) };
        if ret != 0 {
            return Err("VLC 播放启动失败".to_string());
        }

        self.media = Some(media);
        self.player = Some(player);
        Ok(())
    }

    fn stop_inner(&mut self) {
        if let Some(ref libvlc) = self.libvlc {
            // 先停止播放器
            if let Some(ref player) = self.player {
                unsafe { (libvlc.player_stop)(player.0); }
            }
            // 释放资源：media → player（libVLC 内部引用计数）
            if let Some(ref media) = self.media {
                unsafe { (libvlc.media_release)(media.0); }
            }
            if let Some(ref player) = self.player {
                unsafe { (libvlc.player_release)(player.0); }
            }
        }
        self.media = None;
        self.player = None;
    }

    pub fn stop(&mut self) {
        self.stop_inner();
    }

    pub fn shutdown(&mut self) {
        self.stop_inner();
        if let Some(ref libvlc) = self.libvlc {
            if let Some(ref inst) = self.instance {
                unsafe { (libvlc.release)(inst.0); }
            }
        }
        self.instance = None;
        self.libvlc = None;
    }

    #[allow(dead_code)]
    pub fn is_playing(&self) -> bool {
        self.libvlc.as_ref().and_then(|libvlc| {
            self.player.as_ref().map(|p| unsafe { (libvlc.player_is_playing)(p.0) != 0 })
        }).unwrap_or(false)
    }
}

impl Drop for VlcPlayer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

unsafe impl Send for VlcPlayer {}
unsafe impl Sync for VlcPlayer {}

/// Tauri 管理状态
pub struct VlcPlayerState(pub Arc<Mutex<VlcPlayer>>);

unsafe impl Send for VlcPlayerState {}
unsafe impl Sync for VlcPlayerState {}

/// VLC 后台转码器 —— 通过嵌入式 libVLC API 将 RTMP 流转为本地文件，
/// 再通过内建 HTTP 服务器供浏览器播放（带 CORS 支持）
pub struct VlcStreamer {
    libvlc: Option<Libvlc>,
    instance: Option<VlcInstancePtr>,
    media: Option<VlcMediaPtr>,
    player: Option<VlcPlayerPtr>,
    port: u16,
    server_running: Option<Arc<AtomicBool>>,
    server_handle: Option<thread::JoinHandle<()>>,
    temp_file: Option<PathBuf>,
}

impl VlcStreamer {
    pub fn new() -> Self {
        VlcStreamer {
            libvlc: None,
            instance: None,
            media: None,
            player: None,
            port: 0,
            server_running: None,
            server_handle: None,
            temp_file: None,
        }
    }

    /// 启动 libVLC 后台转码：RTMP → 本地 TS 文件 → HTTP 服务器（带 CORS）→ 返回 URL
    pub fn start_stream(&mut self, url: &str) -> Result<String, String> {
        self.stop_stream();

        eprintln!("[vlc] start_stream: url = {}", url);

        let libvlc = if let Some(ref l) = self.libvlc {
            l
        } else {
            eprintln!("[vlc] start_stream: 首次加载 Libvlc::load()...");
            let loaded = Libvlc::load().map_err(|e| {
                let msg = format!("Libvlc::load 失败: {}", e);
                eprintln!("[vlc] {}", msg);
                msg
            })?;
            self.libvlc = Some(loaded);
            self.libvlc.as_ref().unwrap()
        };

        eprintln!("[vlc] start_stream: 调用 create_instance()...");
        let instance = libvlc.create_instance().map_err(|e| {
            let msg = format!("create_instance 失败: {}", e);
            eprintln!("[vlc] {}", msg);
            msg
        })?;
        let inst_ptr = instance.0;

        // 创建临时文件（VLC 将持续写入 TS 数据）
        let temp_file = std::env::temp_dir().join(format!("live48_{}.ts", std::process::id()));
        let temp_path = temp_file.to_string_lossy().to_string();

        // VLC sout：输出到本地文件，减少 mux 延迟
        let sout = format!(
            ":sout=#std{{access=file,mux=ts,dst={}}}",
            temp_path.replace('\\', "/")
        );

        let media = libvlc.create_media(inst_ptr, url)?;
        libvlc.add_media_option(media.0, &sout)?;
        libvlc.add_media_option(media.0, ":sout-keep")?;
        // 减少 TS mux 延迟
        libvlc.add_media_option(media.0, ":sout-ts-dts-delay=40")?;

        let player = libvlc.create_player(inst_ptr)?;
        unsafe {
            (libvlc.player_set_media)(player.0, media.0);
        }
        let ret = unsafe { (libvlc.player_play)(player.0) };
        if ret != 0 {
            unsafe {
                (libvlc.media_release)(media.0);
                (libvlc.player_release)(player.0);
                (libvlc.release)(inst_ptr);
            }
            return Err("VLC 转码启动失败".to_string());
        }

        // 找空闲端口
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();

        let running = Arc::new(AtomicBool::new(true));
        let running_copy = running.clone();
        let file_path = temp_file.clone();

        let handle = thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(200));
            serve_ts_http(listener, file_path, running_copy);
        });

        let local_url = format!("http://127.0.0.1:{}/stream.mpegts", port);

        self.instance = Some(instance);
        self.media = Some(media);
        self.player = Some(player);
        self.port = port;
        self.server_running = Some(running);
        self.server_handle = Some(handle);
        self.temp_file = Some(temp_file);
        Ok(local_url)
    }

    /// 停止 VLC 后台转码 + 停止 HTTP 服务器 + 清理临时文件
    pub fn stop_stream(&mut self) {
        if let Some(running) = self.server_running.take() {
            running.store(false, Ordering::SeqCst);
        }
        if let Some(handle) = self.server_handle.take() {
            let _ = handle.join();
        }
        if let Some(ref path) = self.temp_file {
            let _ = std::fs::remove_file(path);
        }
        self.temp_file = None;

        if let Some(ref libvlc) = self.libvlc {
            if let Some(ref player) = self.player {
                unsafe { (libvlc.player_stop)(player.0); }
            }
            if let Some(ref media) = self.media {
                unsafe { (libvlc.media_release)(media.0); }
            }
            if let Some(ref player) = self.player {
                unsafe { (libvlc.player_release)(player.0); }
            }
            if let Some(ref inst) = self.instance {
                unsafe { (libvlc.release)(inst.0); }
            }
        }
        self.media = None;
        self.player = None;
        self.instance = None;
        self.port = 0;
    }

    /// 检查转码是否正在运行
    pub fn is_running(&self) -> bool {
        self.libvlc.as_ref().and_then(|libvlc| {
            self.player
                .as_ref()
                .map(|p| unsafe { (libvlc.player_is_playing)(p.0) != 0 })
        }).unwrap_or(false)
    }
}

/// 直播录制会话：独立于 VlcStreamer 的一路 libVLC 播放，只走 sout 输出到文件、不上屏。
/// Libvlc 句柄整体 living 在这个结构里（由录制线程独占），因此不需要跨线程传裸指针。
pub struct FileRecorder {
    libvlc: Libvlc,
    instance: VlcInstancePtr,
    media: Option<VlcMediaPtr>,
    player: Option<VlcPlayerPtr>,
    pub output_path: String,
}

impl FileRecorder {
    pub fn start(url: &str, output_path: &str) -> Result<Self, String> {
        let libvlc = Libvlc::load()?;
        let instance = libvlc.create_instance()?;
        let mut recorder = FileRecorder {
            libvlc,
            instance,
            media: None,
            player: None,
            output_path: String::new(),
        };
        recorder.play(url, output_path)?;
        Ok(recorder)
    }

    fn play(&mut self, url: &str, output_path: &str) -> Result<(), String> {
        self.stop_media();

        let sout = format!(
            ":sout=#std{{access=file,mux=ts,dst=\"{}\"}}",
            output_path.replace('\\', "/")
        );
        let media = self.libvlc.create_media(self.instance.0, url)?;
        self.libvlc.add_media_option(media.0, &sout)?;
        self.libvlc.add_media_option(media.0, ":sout-keep")?;
        // 录制这一路不建视频窗口、不开音频，否则每录一条就会弹一个 VLC 窗口并白白解码
        self.libvlc.add_media_option(media.0, ":no-video")?;
        self.libvlc.add_media_option(media.0, ":no-audio")?;

        let player = self.libvlc.create_player(self.instance.0)?;
        unsafe { (self.libvlc.player_set_media)(player.0, media.0) };
        let ret = unsafe { (self.libvlc.player_play)(player.0) };
        if ret != 0 {
            unsafe {
                (self.libvlc.media_release)(media.0);
                (self.libvlc.player_release)(player.0);
            }
            return Err("VLC 录制启动失败".to_string());
        }

        self.media = Some(media);
        self.player = Some(player);
        self.output_path = output_path.to_string();
        Ok(())
    }

    /// 断流后用新地址续录到另一个文件
    pub fn restart(&mut self, url: &str, output_path: &str) -> Result<(), String> {
        self.play(url, output_path)
    }

    pub fn is_playing(&self) -> bool {
        match self.player.as_ref() {
            Some(p) => unsafe { (self.libvlc.player_is_playing)(p.0) != 0 },
            None => false,
        }
    }

    fn stop_media(&mut self) {
        if let Some(ref player) = self.player {
            unsafe { (self.libvlc.player_stop)(player.0) };
        }
        if let Some(ref media) = self.media {
            unsafe { (self.libvlc.media_release)(media.0) };
        }
        if let Some(ref player) = self.player {
            unsafe { (self.libvlc.player_release)(player.0) };
        }
        self.media = None;
        self.player = None;
    }
}

impl Drop for FileRecorder {
    fn drop(&mut self) {
        self.stop_media();
        unsafe { (self.libvlc.release)(self.instance.0) };
    }
}

unsafe impl Send for FileRecorder {}
unsafe impl Sync for FileRecorder {}

/// HTTP 服务器：从 MPEG-TS 文件读取并流式输出，带 CORS 支持
/// 新客户端连上时的起播位置：从文件末尾回看一小段，取窗口内最后一个 H.264 关键帧
/// （优先 SPS，其次 IDR）所在的 TS 包。
/// 直接从末尾开始会让播放器干等下一个关键帧，切换小窗/大窗时的黑屏等待就来自这里。
fn resume_offset(file: &mut std::fs::File) -> u64 {
    const PKT: u64 = 188;
    const LOOKBACK: u64 = 2 * 1024 * 1024;

    let len = match file.metadata() {
        Ok(m) => m.len(),
        Err(_) => return 0,
    };
    if len < PKT * 2 {
        return 0;
    }
    let start = (len - LOOKBACK.min(len)) / PKT * PKT;
    let window = (len - start) as usize;
    let mut buf = vec![0u8; window];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut buf).is_err() {
        return start;
    }

    let mut last_sps = None;
    let mut last_idr = None;
    let mut off = 0usize;
    while off + PKT as usize <= window {
        if buf[off] != 0x47 {
            off += 1; // 丢步后重新找包边界
            continue;
        }
        let pusi = buf[off + 1] & 0x40 != 0;
        let afc = buf[off + 3] >> 4;
        if pusi && afc & 0x02 != 0 {
            let mut i = off + 4;
            if afc & 0x01 != 0 {
                i += 1 + buf[i] as usize; // 跳过 adaptation field
            }
            if i + 10 < window
                && buf[i] == 0
                && buf[i + 1] == 0
                && buf[i + 2] == 0
                && buf[i + 3] == 1
                && (0xE0..=0xEF).contains(&buf[i + 4])
            {
                let nal = i + 9 + buf[i + 8] as usize;
                if nal + 1 < window {
                    match buf[nal] & 0x1F {
                        7 => last_sps = Some(off as u64),
                        5 => last_idr = Some(off as u64),
                        _ => {}
                    }
                }
            }
        }
        off += PKT as usize;
    }
    last_sps.or(last_idr).map(|o| start + o).unwrap_or(start)
}

fn serve_ts_http(
    listener: std::net::TcpListener,
    file_path: PathBuf,
    running: Arc<AtomicBool>,
) {
    let _ = listener.set_nonblocking(true);
    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                // TCP_NODELAY 减少 Nagle 算法带来的延迟
                let _ = stream.set_nodelay(true);
                let _ = stream.set_write_timeout(None);

                // 读取请求（简单解析，判断是否 OPTIONS 预检）
                let mut req_buf = [0u8; 1024];
                let mut is_options = false;
                // 非阻塞读取请求头
                stream.set_nonblocking(false).ok();
                stream.set_read_timeout(Some(std::time::Duration::from_millis(100))).ok();
                if let Ok(n) = stream.read(&mut req_buf) {
                    let req_str = std::str::from_utf8(&req_buf[..n]).unwrap_or("");
                    if req_str.starts_with("OPTIONS") {
                        is_options = true;
                    }
                }
                let _ = stream.set_read_timeout(None);

                if is_options {
                    // CORS 预检响应
                    let resp = b"HTTP/1.1 204 No Content\r\n\
                        Access-Control-Allow-Origin: *\r\n\
                        Access-Control-Allow-Methods: GET, OPTIONS\r\n\
                        Access-Control-Allow-Headers: *\r\n\
                        Access-Control-Max-Age: 86400\r\n\
                        Connection: close\r\n\r\n";
                    let _ = stream.write_all(resp);
                    continue;
                }

                // 带 CORS 头的 HTTP 响应
                let header = b"HTTP/1.1 200 OK\r\n\
                    Content-Type: video/mp2t\r\n\
                    Access-Control-Allow-Origin: *\r\n\
                    Access-Control-Allow-Methods: GET, OPTIONS\r\n\
                    Access-Control-Allow-Headers: *\r\n\
                    Cache-Control: no-cache\r\n\
                    Connection: close\r\n\r\n";
                if stream.write_all(header).is_err() {
                    continue;
                }

                // 等待文件出现
                while running.load(Ordering::SeqCst) && !file_path.exists() {
                    thread::sleep(std::time::Duration::from_millis(10));
                }
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let mut file = match std::fs::File::open(&file_path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                // 从最近的关键帧开始，而不是文件末尾
                let mut pos = resume_offset(&mut file);

                // 大缓冲区减少系统调用次数
                let mut buf = [0u8; 262144]; // 256KB
                let mut idle_count: u32 = 0;
                // BufWriter 批量写入 TCP，减少小包发送
                let mut writer = std::io::BufWriter::with_capacity(131072, &stream);

                while running.load(Ordering::SeqCst) {
                    match file.seek(SeekFrom::Start(pos)) {
                        Ok(_) => {}
                        Err(_) => break,
                    }
                    match file.read(&mut buf) {
                        Ok(0) => {
                            idle_count += 1;
                            let delay = if idle_count < 50 { 1 } else { 3 };
                            thread::sleep(std::time::Duration::from_millis(delay));
                            continue;
                        }
                        Ok(n) => {
                            idle_count = 0;
                            if writer.write_all(&buf[..n]).is_err() {
                                break;
                            }
                            let _ = writer.flush();
                            pos += n as u64;
                        }
                        Err(_) => break,
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            Err(_) => {
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

impl Drop for VlcStreamer {
    fn drop(&mut self) {
        self.stop_stream();
    }
}

unsafe impl Send for VlcStreamer {}
unsafe impl Sync for VlcStreamer {}

/// Tauri 管理状态 —— 包装 Mutex<VlcStreamer>
pub struct VlcStreamState(pub Mutex<VlcStreamer>);

unsafe impl Send for VlcStreamState {}
unsafe impl Sync for VlcStreamState {}
