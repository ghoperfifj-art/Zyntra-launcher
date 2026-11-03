//! macOS counterparts of the Win32 calls: hiding the launcher, the frontmost app and the
//! state of a key — straight through the Objective-C runtime and CoreGraphics, like the
//! Windows side talks to user32 directly.

use std::ffi::{c_void, CStr};

type Id = *mut c_void;
type Sel = *const c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const i8) -> Id;
    fn sel_registerName(name: *const i8) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
}

// libdispatch (part of libSystem): run work on the main thread, where AppKit must be called.
extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

/// kCGEventSourceStateCombinedSessionState: what the user is holding, in any app.
const COMBINED_SESSION: i32 = 0;

fn class(name: &CStr) -> Id {
    // SAFETY: null-terminated class name; returns null for an unknown class.
    unsafe { objc_getClass(name.as_ptr()) }
}

fn sel(name: &CStr) -> Sel {
    // SAFETY: null-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `[obj sel]` returning an object.
fn send(obj: Id, name: &CStr) -> Id {
    if obj.is_null() {
        return obj;
    }
    // SAFETY: objc_msgSend called with the signature of a no-argument method returning id.
    let f: extern "C" fn(Id, Sel) -> Id = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    f(obj, sel(name))
}

/// `[obj sel:arg]` with an object (or nil) argument.
fn send_id(obj: Id, name: &CStr, arg: Id) {
    if obj.is_null() {
        return;
    }
    // SAFETY: as above, for a one-object-argument method returning void.
    let f: extern "C" fn(Id, Sel, Id) = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    f(obj, sel(name), arg)
}

/// `[obj sel:flag]` with a BOOL argument.
fn send_bool(obj: Id, name: &CStr, flag: bool) {
    if obj.is_null() {
        return;
    }
    // SAFETY: as above, for a BOOL-argument method returning void.
    let f: extern "C" fn(Id, Sel, bool) = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    f(obj, sel(name), flag)
}

fn app() -> Id {
    send(class(c"NSApplication"), c"sharedApplication")
}

fn on_main(work: extern "C" fn(*mut c_void)) {
    // SAFETY: the main queue lives for the whole process; `work` ignores its context.
    unsafe { dispatch_async_f(&_dispatch_main_q, std::ptr::null_mut(), work) }
}

extern "C" fn hide_now(_: *mut c_void) {
    send_id(app(), c"hide:", std::ptr::null_mut());
}

extern "C" fn show_now(_: *mut c_void) {
    let app = app();
    send_id(app, c"unhide:", std::ptr::null_mut());
    send_bool(app, c"activateIgnoringOtherApps:", true);
}

/// Hides the whole app (like ⌘H); safe from any thread.
pub fn hide() {
    on_main(hide_now);
}

/// Brings the app back to the front; safe from any thread.
pub fn show() {
    on_main(show_now);
}

/// Process id of the app in front (the one getting the keyboard).
pub fn frontmost_pid() -> i32 {
    // SAFETY: the pool wraps the autoreleased NSRunningApplication this thread receives.
    let pool = unsafe { objc_autoreleasePoolPush() };
    let front = send(send(class(c"NSWorkspace"), c"sharedWorkspace"), c"frontmostApplication");
    let pid = if front.is_null() {
        -1
    } else {
        // SAFETY: processIdentifier returns pid_t (i32) and takes no arguments.
        let f: extern "C" fn(Id, Sel) -> i32 = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        f(front, sel(c"processIdentifier"))
    };
    // SAFETY: pops the pool pushed above, on the same thread.
    unsafe { objc_autoreleasePoolPop(pool) };
    pid
}

/// Whether a key (macOS virtual key code) is held.
pub fn key_down(code: u16) -> bool {
    // SAFETY: plain query of the current input state.
    unsafe { CGEventSourceKeyState(COMBINED_SESSION, code) }
}

/// Whether a mouse button (0 left, 1 right, 2 middle) is held.
pub fn button_down(button: u32) -> bool {
    // SAFETY: as above.
    unsafe { CGEventSourceButtonState(COMBINED_SESSION, button) }
}

/// Whether process `pid` still runs (signal 0 only checks it exists).
pub fn alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SAFETY: signal 0 delivers nothing.
    unsafe { kill(pid as i32, 0) == 0 }
}
