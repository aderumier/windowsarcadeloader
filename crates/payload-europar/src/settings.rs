//! The operator settings the game reads from `data/gamesettings.ini` (`[gamesettings]`),
//! overridden by profile options:
//!
//! * `WAL_EUROPAR_RESOLUTION`: `Resolution`, the full screen mode: 640x480 (the cabinet's),
//!   800x600, 1024x768, 1280x1024 or 1600x1200 (`0x466ac5`; another value: 1600x1200)
//! * `WAL_EUROPAR_FREEPLAY=0|1`: `FreePlay` (the shell needs it too, `shell.rs`)

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

type GetPrivateProfileIntA = unsafe extern "system" fn(*const u8, *const u8, i32, *const u8) -> u32;
type GetPrivateProfileStringA = unsafe extern "system" fn(*const u8, *const u8, *const u8, *mut u8, u32, *const u8) -> u32;

static ORIG_GET_INT: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_STRING: AtomicUsize = AtomicUsize::new(0);

fn key_is(key: *const u8, name: &str) -> bool {
    !key.is_null() && unsafe { std::ffi::CStr::from_ptr(key.cast()) }.to_bytes().eq_ignore_ascii_case(name.as_bytes())
}

fn option(var: &str) -> Option<String> {
    std::env::var(var).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

unsafe extern "system" fn get_private_profile_int(section: *const u8, key: *const u8, default: i32, file: *const u8) -> u32 {
    let f: GetPrivateProfileIntA = unsafe { std::mem::transmute(ORIG_GET_INT.load(Ordering::Relaxed)) };
    let mut v = unsafe { f(section, key, default, file) };
    if key_is(key, "FreePlay") {
        if let Some(o) = option("WAL_EUROPAR_FREEPLAY").and_then(|o| o.parse().ok()) {
            v = o;
        }
        crate::shell::set_free_play(v != 0);
        log!("settings: free play {}", if v != 0 { "on" } else { "off" });
    }
    v
}

unsafe extern "system" fn get_private_profile_string(
    section: *const u8,
    key: *const u8,
    default: *const u8,
    out: *mut u8,
    size: u32,
    file: *const u8,
) -> u32 {
    if key_is(key, "Resolution") && !out.is_null() && size > 0 {
        if let Some(mode) = option("WAL_EUROPAR_RESOLUTION") {
            let n = mode.len().min(size as usize - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(mode.as_ptr(), out, n);
                *out.add(n) = 0;
            }
            log!("settings: resolution {mode}");
            return n as u32;
        }
    }
    let f: GetPrivateProfileStringA = unsafe { std::mem::transmute(ORIG_GET_STRING.load(Ordering::Relaxed)) };
    unsafe { f(section, key, default, out, size, file) }
}

pub(crate) fn init() {
    unsafe {
        if let Some(o) = iat::hook("kernel32.dll", "GetPrivateProfileIntA", get_private_profile_int as *const () as usize) {
            ORIG_GET_INT.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("kernel32.dll", "GetPrivateProfileStringA", get_private_profile_string as *const () as usize) {
            ORIG_GET_STRING.store(o, Ordering::Relaxed);
        }
    }
}
