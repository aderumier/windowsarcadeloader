//! Game window size override: `WAL_WINDOW_SIZE=<w>x<h>` (`screen`: the primary monitor's size).
//!
//! The game's `user32!SetWindowPos` / `MoveWindow` calls that size a top-level window are
//! replaced by `<w>x<h>` at (0,0). In window mode Direct3D stretches the back buffer to the
//! client area, so this also rescales a game drawing a fixed-size back buffer. Crimzon Clover
//! (DxLib) computes a window height of ~5 million pixels, which X refuses (BadAlloc).
//!
//! `WAL_WINDOW_POPUP=1`: the game's top-level windows are created as borderless popups
//! (`WS_POPUP`, no caption, frame or system menu). Shikigami no Shiro III creates a plain
//! overlapped window (style 0: Windows adds a caption and border) and makes its Direct3D 8
//! device fullscreen on it: the window manager showed an empty frame, desktop behind.
//!
//! Always: Direct3D (DXVK's d3d9, also behind its d3d8, and wined3d) minimizes a fullscreen
//! window when it is deactivated. A game started while another fullscreen window keeps the
//! focus (a terminal) was minimized at once, out of the taskbar, and stopped presenting. Their
//! `ShowWindow` minimize requests are dropped: the game keeps running behind, switch to it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetParent, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE};

type HWND = *mut c_void;

static WIDTH: AtomicU32 = AtomicU32::new(0);
static HEIGHT: AtomicU32 = AtomicU32::new(0);
static ORIG_SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_WINDOW_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_WINDOW_W: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    keep_unminimized();
    if std::env::var("WAL_WINDOW_POPUP").is_ok_and(|v| v.trim() == "1") {
        unsafe {
            if let Some(o) = iat::hook("user32.dll", "CreateWindowExA", create_window_a as *const () as usize) {
                ORIG_CREATE_WINDOW_A.store(o, Ordering::Relaxed);
            }
            if let Some(o) = iat::hook("user32.dll", "CreateWindowExW", create_window_w as *const () as usize) {
                ORIG_CREATE_WINDOW_W.store(o, Ordering::Relaxed);
            }
        }
        log!("window: top-level windows created as popups");
    }
    let Some((w, h)) = std::env::var("WAL_WINDOW_SIZE").ok().and_then(|v| {
        if v.trim() == "screen" {
            // primary monitor size (no display mode change in window mode)
            let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            return Some((w as u32, h as u32));
        }
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

const SW_SHOWMINIMIZED: i32 = 2;
const SW_MINIMIZE: i32 = 6;
const SW_SHOWMINNOACTIVE: i32 = 7;
const SW_FORCEMINIMIZE: i32 = 11;

/// Hooks `ShowWindow` in the Direct3D modules already loaded (statically imported by the game).
fn keep_unminimized() {
    for dll in [windows_sys::w!("d3d9.dll"), windows_sys::w!("wined3d.dll")] {
        let base = unsafe { GetModuleHandleW(dll) } as usize;
        if base == 0 {
            continue;
        }
        if let Some(o) = unsafe { iat::hook_module(base, "user32.dll", "ShowWindow", show_window as *const () as usize) } {
            ORIG_SHOW_WINDOW.store(o, Ordering::Relaxed);
        }
    }
}

unsafe extern "system" fn show_window(hwnd: HWND, cmd: i32) -> i32 {
    if matches!(cmd, SW_SHOWMINIMIZED | SW_MINIMIZE | SW_SHOWMINNOACTIVE | SW_FORCEMINIMIZE) {
        log!("window: Direct3D minimize of {hwnd:?} ignored (focus lost)");
        return 1;
    }
    let orig: unsafe extern "system" fn(HWND, i32) -> i32 = unsafe { std::mem::transmute(ORIG_SHOW_WINDOW.load(Ordering::Relaxed)) };
    unsafe { orig(hwnd, cmd) }
}

const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
/// Caption, frame, system menu, minimize/maximize boxes.
const WS_DECORATIONS: u32 = 0x00C0_0000 | 0x0004_0000 | 0x0008_0000 | 0x0002_0000 | 0x0001_0000;
/// Extended styles adding a border.
const WS_EX_DECORATIONS: u32 = 0x0000_0001 | 0x0000_0100 | 0x0000_0200 | 0x0002_0000;

/// The popup style of a top-level window (`parent` null), others unchanged.
fn popup(ex: u32, style: u32, parent: HWND) -> (u32, u32) {
    if !parent.is_null() || style & WS_CHILD != 0 {
        return (ex, style);
    }
    (ex & !WS_EX_DECORATIONS, (style & !WS_DECORATIONS) | WS_POPUP)
}

type CreateWindow<T> = unsafe extern "system" fn(u32, *const T, *const T, u32, i32, i32, i32, i32, HWND, P, P, P) -> HWND;
type P = *mut c_void;

unsafe extern "system" fn create_window_a(
    ex: u32, class: *const u8, title: *const u8, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: P, inst: P, param: P,
) -> HWND {
    let orig: CreateWindow<u8> = unsafe { std::mem::transmute(ORIG_CREATE_WINDOW_A.load(Ordering::Relaxed)) };
    let (ex2, style2) = popup(ex, style, parent);
    if style2 != style {
        log!("window: style {style:#x} ex {ex:#x} -> popup {style2:#x} ex {ex2:#x}");
    }
    unsafe { orig(ex2, class, title, style2, x, y, w, h, parent, menu, inst, param) }
}

unsafe extern "system" fn create_window_w(
    ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: P, inst: P, param: P,
) -> HWND {
    let orig: CreateWindow<u16> = unsafe { std::mem::transmute(ORIG_CREATE_WINDOW_W.load(Ordering::Relaxed)) };
    let (ex2, style2) = popup(ex, style, parent);
    if style2 != style {
        log!("window: style {style:#x} ex {ex:#x} -> popup {style2:#x} ex {ex2:#x}");
    }
    unsafe { orig(ex2, class, title, style2, x, y, w, h, parent, menu, inst, param) }
}
