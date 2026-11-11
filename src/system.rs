//! Physical memory of this PC, used to size the game's RAM limit.

#[cfg(windows)]
#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
}

/// Total physical RAM in MiB (falls back to 8 GiB if Windows doesn't answer).
#[cfg(windows)]
pub fn total_ram_mb() -> u32 {
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `status` is a correctly sized MEMORYSTATUSEX with `length` set.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    if ok && status.total_phys > 0 {
        (status.total_phys / 1_048_576) as u32
    } else {
        8192
    }
}

/// Total physical RAM in MiB from sysctl `hw.memsize` (falls back to 8 GiB).
#[cfg(target_os = "macos")]
pub fn total_ram_mb() -> u32 {
    extern "C" {
        fn sysctlbyname(name: *const i8, old: *mut std::ffi::c_void, old_len: *mut usize, new: *const std::ffi::c_void, new_len: usize) -> i32;
    }
    let mut bytes = 0u64;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: null-terminated name; `bytes` is a u64 and `len` its size, as hw.memsize expects.
    let ok = unsafe { sysctlbyname(c"hw.memsize".as_ptr(), (&mut bytes as *mut u64).cast(), &mut len, std::ptr::null(), 0) } == 0;
    if ok && bytes > 0 {
        (bytes / 1_048_576) as u32
    } else {
        8192
    }
}

/// Most the game may get: everything except ~2 GiB left for the system (min 1 GiB).
pub fn max_game_ram_mb(total: u32) -> u32 {
    (total.saturating_sub(2048) / 512 * 512).max(1024)
}

/// Sensible default: half of the RAM, between 2 and 8 GiB, never above the maximum.
pub fn recommended_ram_mb(total: u32) -> u32 {
    (total / 2 / 512 * 512).clamp(2048, 8192).min(max_game_ram_mb(total))
}

/// Whether Windows' Redirection Guard is on for this process. A launcher started from a program
/// that turned it on (the installer's «Launch» box) inherits it, and so do the games it starts;
/// Horizon then can't open the folder links of its resource pack cache (os error 448) and
/// stops while loading. Only read here: the launcher asks the player to start it again.
#[cfg(windows)]
pub fn redirection_guard() -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessMitigationPolicy(process: isize, policy: i32, buffer: *mut u32, length: usize) -> i32;
    }
    const PROCESS_REDIRECTION_TRUST_POLICY: i32 = 16;
    let mut flags = 0u32;
    // SAFETY: the current process's pseudo handle and a 4-byte buffer, as the call expects.
    let ok = unsafe { GetProcessMitigationPolicy(GetCurrentProcess(), PROCESS_REDIRECTION_TRUST_POLICY, &mut flags, 4) };
    ok != 0 && flags & 1 != 0
}

#[cfg(not(windows))]
pub fn redirection_guard() -> bool {
    false
}
