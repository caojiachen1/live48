//! Win32 window chrome tweaks that egui does not expose.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: *mut c_void, attribute: u32, data: *const u32, size: u32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn ShowWindow(hwnd: *mut c_void, n_cmd_show: i32) -> i32;
}

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const SW_MAXIMIZE: i32 = 3;

/// Dark title bar plus a real maximize: egui's `with_maximized` is silently
/// dropped when the builder also carries an inner size.
pub fn tune(frame: &eframe::Frame) {
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    if let RawWindowHandle::Win32(win) = handle.as_raw() {
        let hwnd = win.hwnd.get() as *mut c_void;
        let on: u32 = 1;
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &on,
                std::mem::size_of::<u32>() as u32,
            );
            ShowWindow(hwnd, SW_MAXIMIZE);
        }
    }
}
