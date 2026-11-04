//! A Unity 4 game's full screen resolution forced to the fake desktop's
//! (`WAL_ADRENALINE_FULLSCREEN_DESKTOP=1`, with `WAL_DESKTOP_SIZE`): Candy Crush Arcade's dump
//! has a patched assembly whose `ApplicationManager.Awake` asks `Screen.SetResolution(1920,
//! 1080, true)` (the original 1080x1920): its portrait picture was drawn landscape, squeezed in
//! the cabinet's portrait window.
//!
//! The Unity 4 player resolves Mono's functions with `GetProcAddress` and registers its internal
//! calls with `mono_add_internal_call`, before any scene: `UnityEngine.Screen::SetResolution`
//! (`(int width, int height, bool fullscreen, int refresh rate)`, cdecl) is registered as the
//! payload's, which passes the desktop's size for a full screen.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{desktop, iat, log};

static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ADD_INTERNAL_CALL: AtomicUsize = AtomicUsize::new(0);
static SET_RESOLUTION: AtomicUsize = AtomicUsize::new(0);

type GetProcAddress = unsafe extern "system" fn(*mut c_void, *const c_char) -> usize;
type AddInternalCall = unsafe extern "C" fn(*const c_char, usize);
type SetResolution = unsafe extern "C" fn(i32, i32, u32, i32);

unsafe extern "C" fn set_resolution(width: i32, height: i32, fullscreen: u32, refresh: i32) {
    let (w, h) = match desktop::size() {
        Some(size) if fullscreen & 0xFF != 0 => size,
        _ => (width, height),
    };
    log!("screen: SetResolution({width}, {height}, {}) -> {w}x{h}", fullscreen & 0xFF != 0);
    let f: SetResolution = unsafe { std::mem::transmute(SET_RESOLUTION.load(Ordering::Relaxed)) };
    unsafe { f(w, h, fullscreen, refresh) };
}

unsafe extern "C" fn add_internal_call(name: *const c_char, method: usize) {
    let f: AddInternalCall = unsafe { std::mem::transmute(ADD_INTERNAL_CALL.load(Ordering::Relaxed)) };
    if !name.is_null() && method != 0 && unsafe { CStr::from_ptr(name) }.to_bytes() == b"UnityEngine.Screen::SetResolution" {
        SET_RESOLUTION.store(method, Ordering::Relaxed);
        log!("screen: Screen.SetResolution full screen at the desktop's size");
        return unsafe { f(name, set_resolution as *const () as usize) };
    }
    unsafe { f(name, method) }
}

unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const c_char) -> usize {
    let f: GetProcAddress = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let r = unsafe { f(module, name) };
    // by name only (an ordinal is a small number)
    if r != 0 && name as usize > 0xFFFF && unsafe { CStr::from_ptr(name) }.to_bytes() == b"mono_add_internal_call" {
        ADD_INTERNAL_CALL.store(r, Ordering::Relaxed);
        return add_internal_call as *const () as usize;
    }
    r
}

/// `exe`: the Unity 4 player (the executable).
pub fn init(exe: usize) {
    if !std::env::var("WAL_ADRENALINE_FULLSCREEN_DESKTOP").is_ok_and(|v| v.trim() == "1") || desktop::size().is_none() {
        return;
    }
    match unsafe { iat::hook_module(exe, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(orig) => GET_PROC_ADDRESS.store(orig, Ordering::Relaxed),
        None => log!("screen: the executable does not import GetProcAddress"),
    }
}
