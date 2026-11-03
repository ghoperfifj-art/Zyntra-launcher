//! OptiFine-style zoom for Horizon, without rebuilding the game: while the zoom key is held
//! over Horizon's window, the launcher narrows the field of view through Horizon's control
//! endpoint (`options-set fov N`, applied at once) and puts it back on release.

#[cfg(windows)]
use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

#[cfg(windows)]
const SYNCHRONIZE: u32 = 0x0010_0000;
#[cfg(windows)]
const WAIT_TIMEOUT: u32 = 0x102;

/// Field of view while zoomed (Horizon's lowest).
pub const ZOOM_FOV: i32 = 30;

/// Keys the zoom can use: (label, virtual-key code).
#[cfg(windows)]
pub const KEYS: [(&str, i32); 5] = [("C", 0x43), ("V", 0x56), ("Z", 0x5A), ("X", 0x58), ("Средняя кнопка мыши", 0x04)];
/// Keys the zoom can use: (label, macOS key code; negative = mouse button -code).
#[cfg(target_os = "macos")]
pub const KEYS: [(&str, i32); 5] = [("C", 8), ("V", 9), ("Z", 6), ("X", 7), ("Средняя кнопка мыши", -2)];

pub fn key_code(label: &str) -> i32 {
    KEYS.iter().find(|(l, _)| *l == label).map_or(KEYS[0].1, |(_, c)| *c)
}

/// Where Horizon writes its control endpoint (`{"port", "token"}`); a `.json` path makes it
/// listen on localhost TCP with a token, which works the same on Windows and macOS.
pub fn endpoint_file(home: &Path) -> PathBuf {
    home.join("zyntra-control.json")
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        CloseHandle(h);
        running
    }
}

#[cfg(windows)]
fn game_focused(pid: u32) -> bool {
    let mut owner = 0u32;
    // SAFETY: GetForegroundWindow may return null, which GetWindowThreadProcessId accepts.
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut owner) };
    owner == pid
}

#[cfg(windows)]
fn key_down(code: i32) -> bool {
    // SAFETY: reads the asynchronous key state; the high bit means "held".
    unsafe { (GetAsyncKeyState(code) as u16 & 0x8000) != 0 }
}

#[cfg(target_os = "macos")]
use crate::mac::alive;

#[cfg(target_os = "macos")]
fn game_focused(pid: u32) -> bool {
    crate::mac::frontmost_pid() == pid as i32
}

#[cfg(target_os = "macos")]
fn key_down(code: i32) -> bool {
    if code < 0 { crate::mac::button_down(-code as u32) } else { crate::mac::key_down(code as u16) }
}

struct Control {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Control {
    fn connect(endpoint: &Path) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(endpoint).ok()?).ok()?;
        let port = v["port"].as_u64()? as u16;
        let token = v["token"].as_str()?;
        let stream = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), Duration::from_secs(2)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        let mut control = Control { reader: BufReader::new(stream.try_clone().ok()?), writer: stream };
        control.send(&format!("auth {token}"))?;
        Some(control)
    }

    fn send(&mut self, line: &str) -> Option<String> {
        writeln!(self.writer, "{line}").ok()?;
        let mut reply = String::new();
        self.reader.read_line(&mut reply).ok()?;
        Some(reply)
    }
}

/// The player's own field of view from options.txt (vanilla keeps it as (fov - 70) / 40).
fn saved_fov(options: &Path) -> i32 {
    std::fs::read_to_string(options)
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("fov:").and_then(|v| v.trim().parse::<f32>().ok())))
        .map(|v| if v.abs() <= 1.0 { (70.0 + v * 40.0).round() as i32 } else { v.round() as i32 })
        .unwrap_or(70)
        .clamp(30, 110)
}

/// Watches the key until the game process exits. Runs on its own thread.
pub fn run(pid: u32, endpoint: PathBuf, options: PathBuf, key: i32) {
    let mut control: Option<Control> = None;
    let mut restore: Option<i32> = None;
    while alive(pid) {
        let want = key_down(key) && game_focused(pid);
        if want != restore.is_some() {
            if control.is_none() {
                control = Control::connect(&endpoint);
            }
            if let Some(c) = control.as_mut() {
                let ok = match restore {
                    None => {
                        // Ask the game for the FOV it uses now; the file is only a fallback. A game
                        // closed mid-zoom may have saved the zoom FOV: then 70.
                        let current = c
                            .send("options-get fov")
                            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
                            .and_then(|v| v["options"]["fov"]["value"].as_i64())
                            .map(|v| v as i32)
                            .unwrap_or_else(|| saved_fov(&options));
                        let original = Some(current).filter(|f| *f > ZOOM_FOV).unwrap_or(70);
                        let sent = c.send(&format!("options-set fov {ZOOM_FOV}")).is_some();
                        if sent {
                            restore = Some(original);
                        }
                        sent
                    }
                    Some(original) => {
                        let sent = c.send(&format!("options-set fov {original}")).is_some();
                        if sent {
                            restore = None;
                        }
                        sent
                    }
                };
                if !ok {
                    control = None; // reconnect next time
                }
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}
