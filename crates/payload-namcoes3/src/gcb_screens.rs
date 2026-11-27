//! Mobile Suit Gundam U.C. Card Builder's two screens. The player's seat (satellite) has two
//! 1920x1080 monitors: the boot waits for two (`GetSystemMetrics(SM_CMONITORS)`), finds two
//! displays attached to the desktop (`EnumDisplayDevicesW`), sets both to 1920x1080
//! (`ChangeDisplaySettingsExW`) and opens a borderless window on each, `\\.\DISPLAY1` (the main
//! screen, at 0,0) and `\\.\DISPLAY2` (the sub screen, at 1920,0), one Direct3D 11 swap chain
//! each. The payload answers two displays (`gcb.rs`) and shows the two screens as
//! `WAL_NAMCOES3_GCB_SCREENS` says:
//!
//! - `split` (default): both in one window over the primary monitor, the main screen on the
//!   left half, the sub screen on the right half. The main window is that window: at each of its
//!   frames its own picture is drawn on the left half and the sub window's last picture on the
//!   right half (`wal_payload_common::subwindow_d3d11`); the sub window is kept off the screen.
//!   Each keeps its 16:9 shape (`WAL_NAMCOES3_GCB_KEEP_ASPECT`, default 1; 0 stretches it over
//!   its half), black around.
//! - `swap`: as `split`, the sub screen on the left.
//! - `monitors`: each screen in its own window over its own monitor (the main screen on the
//!   primary monitor, the sub screen on the next one); with a single monitor, `split`.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::subwindow_d3d11::{self, Layout};
use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN};

/// The game's window places: the main screen at x < 1920, the sub screen at 1920 or more.
const SUB_X: i32 = 1920;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Split,
    Swap,
    Monitors,
}

/// The layout asked for (`monitors` with one monitor: `split`).
pub(crate) fn mode() -> Mode {
    static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        let asked = match std::env::var("WAL_NAMCOES3_GCB_SCREENS").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
            "monitors" => Mode::Monitors,
            "swap" => Mode::Swap,
            _ => Mode::Split,
        };
        if asked == Mode::Monitors && monitors().len() < 2 {
            log!("gcb screens: one monitor, the screens side by side");
            return Mode::Split;
        }
        asked
    })
}

fn keep_aspect() -> bool {
    std::env::var("WAL_NAMCOES3_GCB_KEEP_ASPECT").map(|v| v.trim() != "0").unwrap_or(true)
}

/// The player's monitors (their whole rectangles), the primary first.
fn monitors() -> Vec<RECT> {
    unsafe extern "system" fn each(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> i32 {
        let list = unsafe { &mut *(data as *mut Vec<(RECT, bool)>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            list.push((info.rcMonitor, info.dwFlags & 1 != 0));
        }
        1
    }
    let mut list: Vec<(RECT, bool)> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut list as *mut _ as LPARAM) };
    list.sort_by_key(|(r, primary)| (!primary, r.left, r.top));
    list.into_iter().map(|(r, _)| r).collect()
}

fn primary() -> RECT {
    monitors().first().copied().unwrap_or(RECT { left: 0, top: 0, right: 1920, bottom: 1080 })
}

/// A screen's place in the one window (fractions of it: left, top, width, height): its half,
/// 16:9 in it unless stretched (the window covers the primary monitor: its shape is the
/// monitor's).
fn half(right: bool) -> [f32; 4] {
    let p = primary();
    let (w, h) = ((p.right - p.left) as f32, (p.bottom - p.top) as f32);
    let hw = w / 2.0;
    let (fw, fh) = if !keep_aspect() {
        (hw, h)
    } else if hw * 9.0 > h * 16.0 {
        (h * 16.0 / 9.0, h)
    } else {
        (hw, hw * 9.0 / 16.0)
    };
    let x = if right { hw } else { 0.0 } + (hw - fw) / 2.0;
    let y = (h - fh) / 2.0;
    [x / w, y / h, fw / w, fh / h]
}

/// Where a screen window goes: x, y, w, h.
fn place(sub: bool) -> (i32, i32, i32, i32) {
    match mode() {
        Mode::Monitors => {
            let list = monitors();
            let r = if sub { list[1] } else { list[0] };
            (r.left, r.top, r.right - r.left, r.bottom - r.top)
        }
        _ if sub => {
            // off the screen, right of the virtual screen (its picture drawn in the main window)
            let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) + 64 };
            (x, 0, 1920, 1080)
        }
        _ => {
            let p = primary();
            (p.left, p.top, p.right - p.left, p.bottom - p.top)
        }
    }
}

static CREATE_WINDOW_EX_W: AtomicUsize = AtomicUsize::new(0);
static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static MAIN: AtomicUsize = AtomicUsize::new(0);
static SUB: AtomicUsize = AtomicUsize::new(0);

fn screen_of(hwnd: HWND) -> Option<bool> {
    let h = hwnd as usize;
    if h == 0 {
        None
    } else if h == MAIN.load(Ordering::Relaxed) {
        Some(false)
    } else if h == SUB.load(Ordering::Relaxed) {
        Some(true)
    } else {
        None
    }
}

type CreateW = unsafe extern "system" fn(u32, *const u16, *const u16, u32, i32, i32, i32, i32, HWND, usize, usize, *const core::ffi::c_void) -> HWND;

unsafe extern "system" fn create_window_ex_w(ex: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: usize, inst: usize, param: *const core::ffi::c_void) -> HWND {
    let real: CreateW = unsafe { std::mem::transmute(CREATE_WINDOW_EX_W.load(Ordering::Relaxed)) };
    // the game's screen windows: top-level, 1920x1080 at a display's place
    let screen = parent.is_null() && w == 1920 && h == 1080 && (x == 0 || x == SUB_X) && y == 0;
    if !screen {
        return unsafe { real(ex, class, name, style, x, y, w, h, parent, menu, inst, param) };
    }
    let sub = x >= SUB_X;
    let (nx, ny, nw, nh) = place(sub);
    let hwnd = unsafe { real(ex, class, name, style, nx, ny, nw, nh, parent, menu, inst, param) };
    if !hwnd.is_null() {
        (if sub { &SUB } else { &MAIN }).store(hwnd as usize, Ordering::Relaxed);
    }
    log!("gcb screens: {} screen window {nw}x{nh} at {nx},{ny} ({:?})", if sub { "sub" } else { "main" }, mode());
    hwnd
}

/// The game placing its screen windows (at its displays' places): ours kept.
unsafe extern "system" fn set_window_pos(hwnd: HWND, after: HWND, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32 {
    let real: unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(SET_WINDOW_POS.load(Ordering::Relaxed)) };
    if let Some(sub) = screen_of(hwnd) {
        let (nx, ny, nw, nh) = place(sub);
        return unsafe { real(hwnd, after, nx, ny, nw, nh, flags) };
    }
    unsafe { real(hwnd, after, x, y, w, h, flags) }
}

/// The one window's composition (`split`, `swap`).
fn layout() -> Option<Layout> {
    let (main, sub) = (MAIN.load(Ordering::Relaxed), SUB.load(Ordering::Relaxed));
    if main == 0 || sub == 0 {
        return None;
    }
    let sub_right = mode() == Mode::Split;
    Some(Layout { source: sub, target: main, rect: half(sub_right), pointer: None, target_rect: Some(half(!sub_right)) })
}

/// The game's screen windows (the main one, the sub one), once made.
#[allow(dead_code)]
pub(crate) fn window(sub: bool) -> Option<usize> {
    let h = (if sub { &SUB } else { &MAIN }).load(Ordering::Relaxed);
    (h != 0).then_some(h)
}

/// A screen's place on the player's screen in `split` / `swap` (fractions of the main window),
/// `None` in `monitors` (each screen its whole window).
#[allow(dead_code)]
pub(crate) fn screen_rect(sub: bool) -> Option<[f32; 4]> {
    match mode() {
        Mode::Monitors => None,
        m => Some(half(sub == (m == Mode::Split))),
    }
}

pub(crate) fn init(base: usize) {
    let hooks: [(&str, usize, &AtomicUsize); 2] = [
        ("CreateWindowExW", create_window_ex_w as *const () as usize, &CREATE_WINDOW_EX_W),
        ("SetWindowPos", set_window_pos as *const () as usize, &SET_WINDOW_POS),
    ];
    for (name, f, slot) in hooks {
        match unsafe { iat::hook_module(base, "user32.dll", name, f) } {
            Some(o) => slot.store(o, Ordering::Relaxed),
            None => log!("gcb screens: {name} not hooked"),
        }
    }
    if mode() != Mode::Monitors {
        subwindow_d3d11::init(layout);
    }
    log!("gcb screens: {:?}, {} monitor(s)", mode(), monitors().len());
}
