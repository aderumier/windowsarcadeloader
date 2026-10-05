//! Game window size override: `WAL_WINDOW_SIZE=<w>x<h>`.
//!
//! The game's `user32!SetWindowPos` / `MoveWindow` calls that size a top-level window are
//! replaced by `<w>x<h>` at (0,0). In window mode Direct3D stretches the back buffer to the
//! client area, so this also rescales a game drawing a fixed-size back buffer. Crimzon Clover
//! (DxLib) computes a window height of ~5 million pixels, which X refuses (BadAlloc).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetParent, SWP_NOMOVE, SWP_NOSIZE};

type HWND = *mut c_void;

static WIDTH: AtomicU32 = AtomicU32::new(0);
static HEIGHT: AtomicU32 = AtomicU32::new(0);
static ORIG_SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE_WINDOW: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    let Some((w, h)) = std::env::var("WAL_WINDOW_SIZE").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
    }) else {
        return;
    };
    WIDTH.store(w, Ordering::Relaxed);
    HEIGHT.store(h, Ordering::Relaxed);
    unsafe {
        if let Some(o) = iat::hook("user32.dll", "SetWindowPos", set_window_pos as *const () as usize) {
            ORIG_SET_WINDOW_POS.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("user32.dll", "MoveWindow", move_window as *const () as usize) {
            ORIG_MOVE_WINDOW.store(o, Ordering::Relaxed);
        }
    }
    log!("window: top-level windows sized {w}x{h}");
}

/// The forced size when `hwnd` is a top-level window, logging the replaced geometry.
fn forced(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32) -> Option<(i32, i32)> {
    if !unsafe { GetParent(hwnd) }.is_null() {
        return None;
    }
    let (w, h) = (WIDTH.load(Ordering::Relaxed) as i32, HEIGHT.load(Ordering::Relaxed) as i32);
    if (x, y, cx, cy) != (0, 0, w, h) {
        log!("window: {hwnd:?} {cx}x{cy} at {x},{y} -> {w}x{h} at 0,0");
    }
    Some((w, h))
}

unsafe extern "system" fn set_window_pos(hwnd: HWND, after: HWND, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32 =
        unsafe { std::mem::transmute(ORIG_SET_WINDOW_POS.load(Ordering::Relaxed)) };
    if flags & SWP_NOSIZE == 0 {
        if let Some((w, h)) = forced(hwnd, x, y, cx, cy) {
            return unsafe { orig(hwnd, after, 0, 0, w, h, flags & !SWP_NOMOVE) };
        }
    }
    unsafe { orig(hwnd, after, x, y, cx, cy, flags) }
}

unsafe extern "system" fn move_window(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32, repaint: i32) -> i32 {
    let orig: unsafe extern "system" fn(HWND, i32, i32, i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(ORIG_MOVE_WINDOW.load(Ordering::Relaxed)) };
    match forced(hwnd, x, y, cx, cy) {
        Some((w, h)) => unsafe { orig(hwnd, 0, 0, w, h, repaint) },
        None => unsafe { orig(hwnd, x, y, cx, cy, repaint) },
    }
}
