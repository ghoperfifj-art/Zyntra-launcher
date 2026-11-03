//! Hiding and restoring the launcher window while the game runs (direct Win32 calls,
//! which work from any thread even when the UI is not repainting; on macOS, AppKit's
//! hide/unhide sent to the main thread).

#[cfg(windows)]
use std::ffi::c_void;

#[cfg(windows)]
type Hwnd = *mut c_void;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
}

#[cfg(windows)]
const SW_HIDE: i32 = 0;
#[cfg(windows)]
const SW_RESTORE: i32 = 9;

pub const TITLE: &str = "Zyntra Launcher";

#[cfg(windows)]
fn handle() -> Option<Hwnd> {
    let title: Vec<u16> = TITLE.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: null-terminated UTF-16 title; a null class matches any window class.
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    (!hwnd.is_null()).then_some(hwnd)
}

#[cfg(windows)]
pub fn hide() {
    if let Some(h) = handle() {
        // SAFETY: `h` is a live top-level window handle returned by FindWindowW.
        unsafe { ShowWindow(h, SW_HIDE) };
    }
}

#[cfg(windows)]
pub fn show() {
    if let Some(h) = handle() {
        // SAFETY: as above.
        unsafe {
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
        }
    }
}

#[cfg(target_os = "macos")]
pub use crate::mac::{hide, show};
