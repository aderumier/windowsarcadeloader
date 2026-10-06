//! Work area fix (always on, only when the values are wrong).
//!
//! Under some Wine/Xwayland setups the work area Wine reports is garbage (a height of ~5.4
//! million pixels: `SM_CYFULLSCREEN`, `SM_CYMAXIMIZED`, `SPI_GETWORKAREA`, the monitors'
//! `rcWork`), though the screen size is right. Games sizing their window or back buffer from it
//! fail: Haunted Museum's window 1286x5434821, Gundam's 5434180 pixel wide back buffer (DXVK
//! cannot create it). When the work area is larger than the screen, those calls of the game
//! and of the DLLs in its directory return the screen instead.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXFULLSCREEN, SM_CXMAXIMIZED, SM_CXSCREEN, SM_CYFULLSCREEN, SM_CYMAXIMIZED, SM_CYSCREEN, SPI_GETWORKAREA,
};

type P = *mut c_void;

static ORIG_METRICS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SPI_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_SPI_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_MONITOR_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_MONITOR_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE: AtomicUsize = AtomicUsize::new(0);

/// `WAL_WINDOW_SIZE` for the resizes the game's DLLs make.
unsafe extern "system" fn set_window_pos(hwnd: P, after: P, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(P, P, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(ORIG_SET_POS.load(Ordering::Relaxed)) };
    const SWP_NOSIZE: u32 = 0x1;
    const SWP_NOMOVE: u32 = 0x2;
    if flags & SWP_NOSIZE == 0 {
        if let Some((w, h)) = crate::window::forced(hwnd, x, y, cx, cy) {
            return unsafe { orig(hwnd, after, 0, 0, w, h, flags & !SWP_NOMOVE) };
        }
    }
    unsafe { orig(hwnd, after, x, y, cx, cy, flags) }
}

unsafe extern "system" fn move_window(hwnd: P, x: i32, y: i32, cx: i32, cy: i32, repaint: i32) -> i32 {
    let orig: unsafe extern "system" fn(P, i32, i32, i32, i32, i32) -> i32 = unsafe { std::mem::transmute(ORIG_MOVE.load(Ordering::Relaxed)) };
    match crate::window::forced(hwnd, x, y, cx, cy) {
        Some((w, h)) => unsafe { orig(hwnd, 0, 0, w, h, repaint) },
        None => unsafe { orig(hwnd, x, y, cx, cy, repaint) },
    }
}

/// The game executable and the DLLs of its directory (the payload's excluded).
pub(crate) fn game_modules(exe: usize) -> Vec<usize> {
    let mut found: Vec<(usize, String)> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, 0);
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
        let mut ok = Module32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExePath.iter().position(|c| *c == 0).unwrap_or(0);
            found.push((entry.modBaseAddr as usize, String::from_utf16_lossy(&entry.szExePath[..len]).to_lowercase()));
            ok = Module32NextW(snap, &mut entry) != 0;
        }
        windows_sys::Win32::Foundation::CloseHandle(snap);
    }
    let dir = found.iter().find(|(b, _)| *b == exe).and_then(|(_, p)| p.rsplit_once('\\').map(|(d, _)| format!("{d}\\")));
    let mut modules = vec![exe];
    if let Some(dir) = dir {
        // not the loader's own modules (the payload calls the real functions)
        let ours = |p: &str| p.rsplit('\\').next().is_some_and(|n| n.starts_with("wal_") || n.starts_with("wal-"));
        modules.extend(found.iter().filter(|(b, p)| *b != exe && p.starts_with(&dir) && !ours(p)).map(|(b, _)| *b));
    }
    modules
}

const CW_USEDEFAULT: i32 = i32::MIN;
const WS_CHILD: u32 = 0x4000_0000;

/// A top-level window created at a garbage size (more than 4 screens) or the default size
/// (computed by Wine from the work area): the screen size, at 0,0 when its position is the
/// default one.
fn window_geometry(style: u32, parent: P, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    if !parent.is_null() || style & WS_CHILD != 0 {
        return (x, y, w, h);
    }
    let (sw, sh) = screen();
    let garbage = w == CW_USEDEFAULT || h == CW_USEDEFAULT || w > 4 * sw || h > 4 * sh;
    if !garbage {
        return (x, y, w, h);
    }
    let (nx, ny) = if x == CW_USEDEFAULT { (0, 0) } else { (x, y) };
    let (nw, nh) = (if w == CW_USEDEFAULT || w > 4 * sw { sw } else { w }, if h == CW_USEDEFAULT || h > 4 * sh { sh } else { h });
    log!("workarea: window {w}x{h} at {x},{y} -> {nw}x{nh} at {nx},{ny}");
    (nx, ny, nw, nh)
}

type CreateWindow<T> = unsafe extern "system" fn(u32, *const T, *const T, u32, i32, i32, i32, i32, P, P, P, P) -> P;

unsafe extern "system" fn create_a(
    ex: u32, class: *const u8, title: *const u8, style: u32, x: i32, y: i32, w: i32, h: i32, parent: P, menu: P, inst: P, param: P,
) -> P {
    let orig: CreateWindow<u8> = unsafe { std::mem::transmute(ORIG_CREATE_A.load(Ordering::Relaxed)) };
    let (x, y, w, h) = window_geometry(style, parent, x, y, w, h);
    // the window options (WAL_WINDOW_POPUP / _SIZE) for windows the game's DLLs create
    let (ex, style, x, y, w, h) = crate::window::created(ex, style, x, y, w, h, parent);
    unsafe { orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param) }
}

unsafe extern "system" fn create_w(
    ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: P, menu: P, inst: P, param: P,
) -> P {
    let orig: CreateWindow<u16> = unsafe { std::mem::transmute(ORIG_CREATE_W.load(Ordering::Relaxed)) };
    let (x, y, w, h) = window_geometry(style, parent, x, y, w, h);
    // the window options (WAL_WINDOW_POPUP / _SIZE) for windows the game's DLLs create
    let (ex, style, x, y, w, h) = crate::window::created(ex, style, x, y, w, h, parent);
    unsafe { orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param) }
}

fn screen() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

pub fn init() {
    let (w, h) = screen();
    let (fw, fh) = unsafe { (GetSystemMetrics(SM_CXFULLSCREEN), GetSystemMetrics(SM_CYFULLSCREEN)) };
    if w <= 0 || h <= 0 || (fw <= w && fh <= h) {
        return;
    }
    log!("workarea: Wine reports {fw}x{fh} for a {w}x{h} screen: the screen is used");
    // the game and the DLLs of its directory (game engines query it themselves)
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let modules = game_modules(exe);
    let hooks: [(&str, usize, &AtomicUsize); 9] = [
        ("SetWindowPos", set_window_pos as *const () as usize, &ORIG_SET_POS),
        ("MoveWindow", move_window as *const () as usize, &ORIG_MOVE),
        ("CreateWindowExA", create_a as *const () as usize, &ORIG_CREATE_A),
        ("CreateWindowExW", create_w as *const () as usize, &ORIG_CREATE_W),
        ("GetSystemMetrics", metrics as *const () as usize, &ORIG_METRICS),
        ("SystemParametersInfoA", spi_a as *const () as usize, &ORIG_SPI_A),
        ("SystemParametersInfoW", spi_w as *const () as usize, &ORIG_SPI_W),
        ("GetMonitorInfoA", monitor_a as *const () as usize, &ORIG_MONITOR_A),
        ("GetMonitorInfoW", monitor_w as *const () as usize, &ORIG_MONITOR_W),
    ];
    for &base in &modules {
        for (name, f, orig) in &hooks {
            if let Some(o) = unsafe { iat::hook_module(base, "user32.dll", name, *f) } {
                if o != *f {
                    orig.store(o, Ordering::Relaxed);
                }
            }
        }
    }
    log!("workarea: {} module(s) fixed", modules.len());
}

unsafe extern "system" fn metrics(index: i32) -> i32 {
    let orig: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(ORIG_METRICS.load(Ordering::Relaxed)) };
    let (w, h) = screen();
    match index {
        SM_CXFULLSCREEN | SM_CXMAXIMIZED => w,
        SM_CYFULLSCREEN | SM_CYMAXIMIZED => h,
        _ => unsafe { orig(index) },
    }
}

fn fix_rect(r: *mut RECT) {
    if r.is_null() {
        return;
    }
    let (w, h) = screen();
    unsafe {
        if (*r).right - (*r).left > w || (*r).bottom - (*r).top > h {
            *r = RECT { left: 0, top: 0, right: w, bottom: h };
        }
    }
}

unsafe extern "system" fn spi_a(action: u32, param: u32, pv: P, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(u32, u32, P, u32) -> i32 = unsafe { std::mem::transmute(ORIG_SPI_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(action, param, pv, flags) };
    if action == SPI_GETWORKAREA {
        fix_rect(pv.cast());
    }
    r
}

unsafe extern "system" fn spi_w(action: u32, param: u32, pv: P, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(u32, u32, P, u32) -> i32 = unsafe { std::mem::transmute(ORIG_SPI_W.load(Ordering::Relaxed)) };
    let r = unsafe { orig(action, param, pv, flags) };
    if action == SPI_GETWORKAREA {
        fix_rect(pv.cast());
    }
    r
}

/// MONITORINFO: cbSize, rcMonitor, rcWork (at +20), dwFlags.
unsafe extern "system" fn monitor_a(monitor: P, info: *mut u8) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut u8) -> i32 = unsafe { std::mem::transmute(ORIG_MONITOR_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(monitor, info) };
    if r != 0 && !info.is_null() {
        fix_rect(unsafe { info.add(20) }.cast());
    }
    r
}

unsafe extern "system" fn monitor_w(monitor: P, info: *mut u8) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut u8) -> i32 = unsafe { std::mem::transmute(ORIG_MONITOR_W.load(Ordering::Relaxed)) };
    let r = unsafe { orig(monitor, info) };
    if r != 0 && !info.is_null() {
        fix_rect(unsafe { info.add(20) }.cast());
    }
    r
}
