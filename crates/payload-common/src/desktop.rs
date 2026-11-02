//! A desktop of another size for the game: `WAL_DESKTOP_SIZE=<w>x<h>`.
//!
//! Portrait cabinet games (Top Gun: Unity at 1080x1920) render at the size of their monitor,
//! and Unity makes no window taller than the desktop: on a 1920x1080 screen the game rendered
//! 1080x1080 and its UI, laid out for 1080x1920, was cut. The game and the DLLs of its
//! directory see a `<w>x<h>` desktop instead (`GetSystemMetrics`, the monitors' areas, the
//! work area, the display modes), and their top-level windows, sized and placed on it, are
//! fitted onto the real screen (aspect kept, centered). Direct3D stretches the back buffer the
//! game renders at its own size to that smaller window. DXGI (DXVK) resizes the window to the
//! back buffer itself (`ResizeTarget`: `MoveWindow`): its imports are hooked too. The windows'
//! size limits (`WM_GETMINMAXINFO`, Wine's X size hints) are fitted too. The d3d9 shim reports
//! the desktop as the display mode (`GetAdapterDisplayMode`, the device's `GetDisplayMode`).
//! A mode change to the desktop's size (`ChangeDisplaySettings*`: Unity 5's full screen) is done
//! without changing anything: a portrait mode is none of a landscape screen's (Spider-Man
//! Homecoming, 1080x1920: a black screen on NVIDIA's drivers); `GetDeviceCaps` gives the
//! desktop's size.
//!
//! `WAL_DESKTOP_CLIENT=1`: the top-level windows' client areas answer in desktop units too
//! (`GetClientRect`: the fitted window's client size unscaled, `GetWindowRect`, `ClientToScreen`:
//! places on the desktop). Harikiri Professional Baseball's test menu makes its back buffer and then its window
//! the client size it reads (scaled again on each turn otherwise), its game subtracts its
//! window's origin from its touch position, already in desktop coordinates.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{DEVMODEA, DEVMODEW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXFULLSCREEN, SM_CXMAXIMIZED, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYFULLSCREEN, SM_CYMAXIMIZED, SM_CYSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SPI_GETWORKAREA,
};

type P = *mut c_void;

/// The game's desktop size.
static SIZE: OnceLock<(i32, i32)> = OnceLock::new();

/// The desktop size the game sees (`WAL_DESKTOP_SIZE`), if any.
pub fn size() -> Option<(i32, i32)> {
    SIZE.get().copied()
}

static ORIG_METRICS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SPI_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_SPI_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_MONITOR_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_MONITOR_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENUM_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENUM_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENUM_EX_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENUM_EX_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_CHANGE_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CHANGE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_CHANGE_EX_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CHANGE_EX_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_DEVICE_CAPS: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLIENT_TO_SCREEN: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLIENT_RECT: AtomicUsize = AtomicUsize::new(0);
static ORIG_WINDOW_RECT: AtomicUsize = AtomicUsize::new(0);

fn screen() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// The real screen's size (the payload's own calls are not hooked).
pub fn screen_size() -> (i32, i32) {
    screen()
}

/// The game's desktop on the real screen: scale, left and top offsets.
fn fit() -> (f64, f64, f64) {
    let (dw, dh) = size().unwrap_or(screen());
    let (sw, sh) = screen();
    let scale = (sw as f64 / dw as f64).min(sh as f64 / dh as f64);
    (scale, (sw as f64 - dw as f64 * scale) / 2.0, (sh as f64 - dh as f64 * scale) / 2.0)
}

/// A window of the game's desktop on the real screen: its size scaled, centered (one game
/// window; the positions callers pass back may come from `GetWindowRect`, real ones).
fn to_screen(x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let (s, _, _) = fit();
    let (sw, sh) = screen();
    let (nw, nh) = ((w as f64 * s).round() as i32, (h as f64 * s).round() as i32);
    let (nx, ny) = ((sw - nw) / 2, (sh - nh) / 2);
    log!("desktop: window {w}x{h} at {x},{y} -> {nw}x{nh} at {nx},{ny}");
    (nx, ny, nw, nh)
}

const CW_USEDEFAULT: i32 = i32::MIN;
const WS_CHILD: u32 = 0x4000_0000;
const SWP_NOSIZE: u32 = 0x1;
const SWP_NOMOVE: u32 = 0x2;

fn top_level(parent: P, style: u32) -> bool {
    parent.is_null() && style & WS_CHILD == 0
}

type CreateWindow<T> = unsafe extern "system" fn(u32, *const T, *const T, u32, i32, i32, i32, i32, P, P, P, P) -> P;

/// A window created with a size, top-level: fitted.
fn created(style: u32, parent: P, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    if !top_level(parent, style) || w == CW_USEDEFAULT || w <= 0 || h <= 0 {
        return (x, y, w, h);
    }
    let (x, y) = if x == CW_USEDEFAULT { (0, 0) } else { (x, y) };
    to_screen(x, y, w, h)
}

unsafe extern "system" fn create_a(
    ex: u32, class: *const u8, title: *const u8, style: u32, x: i32, y: i32, w: i32, h: i32, parent: P, menu: P, inst: P, param: P,
) -> P {
    let orig: CreateWindow<u8> = unsafe { std::mem::transmute(ORIG_CREATE_A.load(Ordering::Relaxed)) };
    let (x, y, w, h) = created(style, parent, x, y, w, h);
    let hwnd = unsafe { orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param) };
    if !hwnd.is_null() && top_level(parent, style) {
        subclass(hwnd);
    }
    hwnd
}

unsafe extern "system" fn create_w(
    ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: P, menu: P, inst: P, param: P,
) -> P {
    let orig: CreateWindow<u16> = unsafe { std::mem::transmute(ORIG_CREATE_W.load(Ordering::Relaxed)) };
    let (x, y, w, h) = created(style, parent, x, y, w, h);
    let hwnd = unsafe { orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param) };
    if !hwnd.is_null() && top_level(parent, style) {
        subclass(hwnd);
    }
    hwnd
}

fn is_top_level(hwnd: P) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(hwnd) }.is_null()
}

unsafe extern "system" fn set_window_pos(hwnd: P, after: P, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(P, P, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(ORIG_SET_POS.load(Ordering::Relaxed)) };
    if is_top_level(hwnd) && flags & SWP_NOSIZE == 0 {
        let (nx, ny, nw, nh) = to_screen(if flags & SWP_NOMOVE != 0 { 0 } else { x }, if flags & SWP_NOMOVE != 0 { 0 } else { y }, cx, cy);
        return unsafe { orig(hwnd, after, nx, ny, nw, nh, flags & !SWP_NOMOVE) };
    }
    unsafe { orig(hwnd, after, x, y, cx, cy, flags) }
}

unsafe extern "system" fn move_window(hwnd: P, x: i32, y: i32, cx: i32, cy: i32, repaint: i32) -> i32 {
    let orig: unsafe extern "system" fn(P, i32, i32, i32, i32, i32) -> i32 = unsafe { std::mem::transmute(ORIG_MOVE.load(Ordering::Relaxed)) };
    if is_top_level(hwnd) {
        let (nx, ny, nw, nh) = to_screen(x, y, cx, cy);
        return unsafe { orig(hwnd, nx, ny, nw, nh, repaint) };
    }
    unsafe { orig(hwnd, x, y, cx, cy, repaint) }
}

/// `ClientToScreen` (`WAL_DESKTOP_CLIENT`): a point of the game's desktop (its fitted window's
/// client origin is the desktop's top left).
unsafe extern "system" fn client_to_screen(hwnd: P, point: *mut windows_sys::Win32::Foundation::POINT) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut windows_sys::Win32::Foundation::POINT) -> i32 =
        unsafe { std::mem::transmute(ORIG_CLIENT_TO_SCREEN.load(Ordering::Relaxed)) };
    let ok = unsafe { orig(hwnd, point) };
    if ok != 0 && !point.is_null() {
        let p = unsafe { &mut *point };
        let (x, y) = from_screen(p.x as f64, p.y as f64);
        (p.x, p.y) = (x.round() as i32, y.round() as i32);
    }
    ok
}

/// `GetClientRect` (`WAL_DESKTOP_CLIENT`): a top-level window's client size in desktop units.
unsafe extern "system" fn client_rect(hwnd: P, rect: *mut RECT) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut RECT) -> i32 = unsafe { std::mem::transmute(ORIG_CLIENT_RECT.load(Ordering::Relaxed)) };
    let ok = unsafe { orig(hwnd, rect) };
    if ok != 0 && !rect.is_null() && is_top_level(hwnd) {
        let (s, _, _) = fit();
        let r = unsafe { &mut *rect };
        r.right = r.left + (f64::from(r.right - r.left) / s).round() as i32;
        r.bottom = r.top + (f64::from(r.bottom - r.top) / s).round() as i32;
    }
    ok
}

/// `GetWindowRect` (`WAL_DESKTOP_CLIENT`): a top-level window's place on the desktop.
unsafe extern "system" fn window_rect(hwnd: P, rect: *mut RECT) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut RECT) -> i32 = unsafe { std::mem::transmute(ORIG_WINDOW_RECT.load(Ordering::Relaxed)) };
    let ok = unsafe { orig(hwnd, rect) };
    if ok != 0 && !rect.is_null() && is_top_level(hwnd) {
        let r = unsafe { &mut *rect };
        let (left, top) = from_screen(r.left as f64, r.top as f64);
        let (right, bottom) = from_screen(r.right as f64, r.bottom as f64);
        (r.left, r.top, r.right, r.bottom) = (left.round() as i32, top.round() as i32, right.round() as i32, bottom.round() as i32);
    }
    ok
}

unsafe extern "system" fn metrics(index: i32) -> i32 {
    let orig: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(ORIG_METRICS.load(Ordering::Relaxed)) };
    let (w, h) = size().unwrap_or((0, 0));
    match index {
        SM_CXSCREEN | SM_CXFULLSCREEN | SM_CXMAXIMIZED | SM_CXVIRTUALSCREEN => w,
        SM_CYSCREEN | SM_CYFULLSCREEN | SM_CYMAXIMIZED | SM_CYVIRTUALSCREEN => h,
        SM_XVIRTUALSCREEN | SM_YVIRTUALSCREEN => 0,
        _ => unsafe { orig(index) },
    }
}

fn desktop_rect(r: *mut RECT) {
    if let (false, Some((w, h))) = (r.is_null(), size()) {
        unsafe { *r = RECT { left: 0, top: 0, right: w, bottom: h } };
    }
}

unsafe extern "system" fn spi_a(action: u32, param: u32, pv: P, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(u32, u32, P, u32) -> i32 = unsafe { std::mem::transmute(ORIG_SPI_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(action, param, pv, flags) };
    if action == SPI_GETWORKAREA {
        desktop_rect(pv.cast());
    }
    r
}

unsafe extern "system" fn spi_w(action: u32, param: u32, pv: P, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(u32, u32, P, u32) -> i32 = unsafe { std::mem::transmute(ORIG_SPI_W.load(Ordering::Relaxed)) };
    let r = unsafe { orig(action, param, pv, flags) };
    if action == SPI_GETWORKAREA {
        desktop_rect(pv.cast());
    }
    r
}

/// MONITORINFO: cbSize, rcMonitor (+4), rcWork (+20), dwFlags.
fn monitor_rects(info: *mut u8) {
    if !info.is_null() {
        desktop_rect(unsafe { info.add(4) }.cast());
        desktop_rect(unsafe { info.add(20) }.cast());
    }
}

unsafe extern "system" fn monitor_a(monitor: P, info: *mut u8) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut u8) -> i32 = unsafe { std::mem::transmute(ORIG_MONITOR_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(monitor, info) };
    if r != 0 {
        monitor_rects(info);
    }
    r
}

unsafe extern "system" fn monitor_w(monitor: P, info: *mut u8) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut u8) -> i32 = unsafe { std::mem::transmute(ORIG_MONITOR_W.load(Ordering::Relaxed)) };
    let r = unsafe { orig(monitor, info) };
    if r != 0 {
        monitor_rects(info);
    }
    r
}

/// The window procedures replaced by [`window_proc`]: window, original.
static PROCS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Puts [`window_proc`] in front of a top-level window's procedure.
fn subclass(hwnd: P) {
    use windows_sys::Win32::UI::WindowsAndMessaging::GWLP_WNDPROC;
    #[cfg(target_pointer_width = "64")]
    use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW as set_long;
    #[cfg(target_pointer_width = "32")]
    use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongW as set_long;
    let mut procs = PROCS.lock().unwrap();
    if procs.iter().any(|(h, _)| *h == hwnd as usize) {
        return;
    }
    let orig = unsafe { set_long(hwnd, GWLP_WNDPROC, window_proc as *const () as usize as _) } as usize;
    if orig != 0 {
        procs.push((hwnd as usize, orig));
    }
}

/// The window's size limits (`WM_GETMINMAXINFO`, which Wine gives the window manager) and
/// the window's moves (`WM_WINDOWPOSCHANGING`, in which Unity puts its render size back): those
/// beyond the real screen are fitted as the window.
unsafe extern "system" fn window_proc(hwnd: P, msg: u32, wparam: usize, lparam: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallWindowProcW, WM_GETMINMAXINFO};
    let Some(orig) = PROCS.lock().unwrap().iter().find(|(h, _)| *h == hwnd as usize).map(|(_, o)| *o) else {
        return 0;
    };
    let r = unsafe { CallWindowProcW(Some(std::mem::transmute::<usize, _>(orig)), hwnd, msg, wparam, lparam) };
    // Unity's procedure puts its own size back (WM_WINDOWPOSCHANGING): fitted again
    if msg == windows_sys::Win32::UI::WindowsAndMessaging::WM_WINDOWPOSCHANGING && lparam != 0 {
        let pos = lparam as *mut windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPOS;
        let (sw, sh) = screen();
        unsafe {
            if (*pos).flags & SWP_NOSIZE == 0 && ((*pos).cx > sw || (*pos).cy > sh) {
                let (x, y, w, h) = to_screen((*pos).x, (*pos).y, (*pos).cx, (*pos).cy);
                ((*pos).x, (*pos).y, (*pos).cx, (*pos).cy) = (x, y, w, h);
                (*pos).flags &= !SWP_NOMOVE;
            }
        }
    }
    if msg == WM_GETMINMAXINFO && lparam != 0 {
        let (s, _, _) = fit();
        let (sw, sh) = screen();
        // MINMAXINFO: reserved, max size (+8), max position, min track size (+24), max track (+32)
        for offset in [8, 24, 32] {
            let point = (lparam as usize + offset) as *mut [i32; 2];
            let [x, y] = unsafe { point.read_unaligned() };
            if x > sw || y > sh {
                unsafe { point.write_unaligned([(x as f64 * s).round() as i32, (y as f64 * s).round() as i32]) };
            }
        }
    }
    r
}

/// The top-level windows of this process already created: subclassed.
fn subclass_existing() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};
    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() } {
            subclass(hwnd);
        }
        1
    }
    unsafe { EnumWindows(Some(each), 0) };
}

/// The display modes: the current one and the only one, the desktop's size.
const ENUM_CURRENT_SETTINGS: u32 = u32::MAX;
const ENUM_REGISTRY_SETTINGS: u32 = u32::MAX - 1;

fn only_mode(mode: u32) -> bool {
    matches!(mode, 0 | ENUM_CURRENT_SETTINGS | ENUM_REGISTRY_SETTINGS)
}

unsafe extern "system" fn enum_a(device: *const u8, mode: u32, dm: *mut DEVMODEA) -> i32 {
    let orig: unsafe extern "system" fn(*const u8, u32, *mut DEVMODEA) -> i32 = unsafe { std::mem::transmute(ORIG_ENUM_A.load(Ordering::Relaxed)) };
    if !only_mode(mode) {
        return 0;
    }
    let r = unsafe { orig(device, ENUM_CURRENT_SETTINGS, dm) };
    if let (true, false, Some((w, h))) = (r != 0, dm.is_null(), size()) {
        unsafe { ((*dm).dmPelsWidth, (*dm).dmPelsHeight) = (w as u32, h as u32) };
    }
    r
}

unsafe extern "system" fn enum_w(device: *const u16, mode: u32, dm: *mut DEVMODEW) -> i32 {
    let orig: unsafe extern "system" fn(*const u16, u32, *mut DEVMODEW) -> i32 = unsafe { std::mem::transmute(ORIG_ENUM_W.load(Ordering::Relaxed)) };
    if !only_mode(mode) {
        return 0;
    }
    let r = unsafe { orig(device, ENUM_CURRENT_SETTINGS, dm) };
    if let (true, false, Some((w, h))) = (r != 0, dm.is_null(), size()) {
        unsafe { ((*dm).dmPelsWidth, (*dm).dmPelsHeight) = (w as u32, h as u32) };
    }
    r
}

unsafe extern "system" fn enum_ex_a(device: *const u8, mode: u32, dm: *mut DEVMODEA, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(*const u8, u32, *mut DEVMODEA, u32) -> i32 =
        unsafe { std::mem::transmute(ORIG_ENUM_EX_A.load(Ordering::Relaxed)) };
    if !only_mode(mode) {
        return 0;
    }
    let r = unsafe { orig(device, ENUM_CURRENT_SETTINGS, dm, flags) };
    if let (true, false, Some((w, h))) = (r != 0, dm.is_null(), size()) {
        unsafe { ((*dm).dmPelsWidth, (*dm).dmPelsHeight) = (w as u32, h as u32) };
    }
    r
}

unsafe extern "system" fn enum_ex_w(device: *const u16, mode: u32, dm: *mut DEVMODEW, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(*const u16, u32, *mut DEVMODEW, u32) -> i32 =
        unsafe { std::mem::transmute(ORIG_ENUM_EX_W.load(Ordering::Relaxed)) };
    if !only_mode(mode) {
        return 0;
    }
    let r = unsafe { orig(device, ENUM_CURRENT_SETTINGS, dm, flags) };
    if let (true, false, Some((w, h))) = (r != 0, dm.is_null(), size()) {
        unsafe { ((*dm).dmPelsWidth, (*dm).dmPelsHeight) = (w as u32, h as u32) };
    }
    r
}

/// A mode change to the desktop's own size (a portrait game's full screen: no mode of the real
/// screen, NVIDIA's drivers give a black screen): done, nothing changed. Others: the real call.
const DISP_CHANGE_SUCCESSFUL: i32 = 0;
const DM_PELSWIDTH: u32 = 0x0008_0000;
const DM_PELSHEIGHT: u32 = 0x0010_0000;

fn desktop_mode(fields: u32, w: u32, h: u32) -> bool {
    let Some((dw, dh)) = size() else { return false };
    (fields & DM_PELSWIDTH == 0 || w == dw as u32) && (fields & DM_PELSHEIGHT == 0 || h == dh as u32)
}

unsafe extern "system" fn change_a(dm: *const DEVMODEA, flags: u32) -> i32 {
    if dm.is_null() || unsafe { desktop_mode((*dm).dmFields, (*dm).dmPelsWidth, (*dm).dmPelsHeight) } {
        return DISP_CHANGE_SUCCESSFUL;
    }
    let orig: unsafe extern "system" fn(*const DEVMODEA, u32) -> i32 = unsafe { std::mem::transmute(ORIG_CHANGE_A.load(Ordering::Relaxed)) };
    unsafe { orig(dm, flags) }
}

unsafe extern "system" fn change_w(dm: *const DEVMODEW, flags: u32) -> i32 {
    if dm.is_null() || unsafe { desktop_mode((*dm).dmFields, (*dm).dmPelsWidth, (*dm).dmPelsHeight) } {
        return DISP_CHANGE_SUCCESSFUL;
    }
    let orig: unsafe extern "system" fn(*const DEVMODEW, u32) -> i32 = unsafe { std::mem::transmute(ORIG_CHANGE_W.load(Ordering::Relaxed)) };
    unsafe { orig(dm, flags) }
}

unsafe extern "system" fn change_ex_a(device: *const u8, dm: *const DEVMODEA, hwnd: P, flags: u32, param: P) -> i32 {
    if dm.is_null() || unsafe { desktop_mode((*dm).dmFields, (*dm).dmPelsWidth, (*dm).dmPelsHeight) } {
        return DISP_CHANGE_SUCCESSFUL;
    }
    let orig: unsafe extern "system" fn(*const u8, *const DEVMODEA, P, u32, P) -> i32 = unsafe { std::mem::transmute(ORIG_CHANGE_EX_A.load(Ordering::Relaxed)) };
    unsafe { orig(device, dm, hwnd, flags, param) }
}

unsafe extern "system" fn change_ex_w(device: *const u16, dm: *const DEVMODEW, hwnd: P, flags: u32, param: P) -> i32 {
    if dm.is_null() || unsafe { desktop_mode((*dm).dmFields, (*dm).dmPelsWidth, (*dm).dmPelsHeight) } {
        return DISP_CHANGE_SUCCESSFUL;
    }
    let orig: unsafe extern "system" fn(*const u16, *const DEVMODEW, P, u32, P) -> i32 = unsafe { std::mem::transmute(ORIG_CHANGE_EX_W.load(Ordering::Relaxed)) };
    unsafe { orig(device, dm, hwnd, flags, param) }
}

/// `GetDeviceCaps`: the screen's size in pixels, the desktop's.
unsafe extern "system" fn device_caps(dc: P, index: i32) -> i32 {
    const HORZRES: i32 = 8;
    const VERTRES: i32 = 10;
    const DESKTOPVERTRES: i32 = 117;
    const DESKTOPHORZRES: i32 = 118;
    if let Some((w, h)) = size() {
        match index {
            HORZRES | DESKTOPHORZRES => return w,
            VERTRES | DESKTOPVERTRES => return h,
            _ => {}
        }
    }
    let orig: unsafe extern "system" fn(P, i32) -> i32 = unsafe { std::mem::transmute(ORIG_DEVICE_CAPS.load(Ordering::Relaxed)) };
    unsafe { orig(dc, index) }
}

pub fn init() {
    let Some((w, h)) = std::env::var("WAL_DESKTOP_SIZE").ok().and_then(|v| {
        let (w, h) = v.trim().split_once('x')?;
        Some((w.trim().parse::<i32>().ok()?, h.trim().parse::<i32>().ok()?))
    }) else {
        return;
    };
    if w <= 0 || h <= 0 {
        return;
    }
    let _ = SIZE.set((w, h));
    // the game and the DLLs of its directory (Unity's player)
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let mut modules = crate::workarea::game_modules(exe);
    // the swap chain's window resizes (DXVK's ResizeTarget: MoveWindow) and monitor queries
    for dll in [windows_sys::w!("dxgi.dll"), windows_sys::w!("d3d11.dll")] {
        let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(dll) } as usize;
        if base != 0 && !modules.contains(&base) {
            modules.push(base);
        }
    }
    let client = std::env::var("WAL_DESKTOP_CLIENT").is_ok_and(|v| v.trim() == "1");
    let hooks: [(&str, usize, &AtomicUsize); 20] = [
        ("GetWindowRect", if client { window_rect as *const () as usize } else { 0 }, &ORIG_WINDOW_RECT),
        ("ClientToScreen", if client { client_to_screen as *const () as usize } else { 0 }, &ORIG_CLIENT_TO_SCREEN),
        ("GetClientRect", if client { client_rect as *const () as usize } else { 0 }, &ORIG_CLIENT_RECT),
        ("SetWindowPos", set_window_pos as *const () as usize, &ORIG_SET_POS),
        ("MoveWindow", move_window as *const () as usize, &ORIG_MOVE),
        ("CreateWindowExA", create_a as *const () as usize, &ORIG_CREATE_A),
        ("CreateWindowExW", create_w as *const () as usize, &ORIG_CREATE_W),
        ("GetSystemMetrics", metrics as *const () as usize, &ORIG_METRICS),
        ("SystemParametersInfoA", spi_a as *const () as usize, &ORIG_SPI_A),
        ("SystemParametersInfoW", spi_w as *const () as usize, &ORIG_SPI_W),
        ("GetMonitorInfoA", monitor_a as *const () as usize, &ORIG_MONITOR_A),
        ("GetMonitorInfoW", monitor_w as *const () as usize, &ORIG_MONITOR_W),
        ("EnumDisplaySettingsA", enum_a as *const () as usize, &ORIG_ENUM_A),
        ("EnumDisplaySettingsW", enum_w as *const () as usize, &ORIG_ENUM_W),
        ("EnumDisplaySettingsExA", enum_ex_a as *const () as usize, &ORIG_ENUM_EX_A),
        ("EnumDisplaySettingsExW", enum_ex_w as *const () as usize, &ORIG_ENUM_EX_W),
        ("ChangeDisplaySettingsA", change_a as *const () as usize, &ORIG_CHANGE_A),
        ("ChangeDisplaySettingsW", change_w as *const () as usize, &ORIG_CHANGE_W),
        ("ChangeDisplaySettingsExA", change_ex_a as *const () as usize, &ORIG_CHANGE_EX_A),
        ("ChangeDisplaySettingsExW", change_ex_w as *const () as usize, &ORIG_CHANGE_EX_W),
    ];
    let user32 = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(windows_sys::w!("user32.dll")) };
    for (name, f, orig) in hooks.iter().filter(|h| h.1 != 0) {
        for &base in &modules {
            if let Some(o) = unsafe { iat::hook_module(base, "user32.dll", name, *f) } {
                if o != *f {
                    orig.store(o, Ordering::Relaxed);
                }
            }
        }
        // not imported by these modules: the original for the others' calls
        if orig.load(Ordering::Relaxed) == 0 {
            let cname = format!("{name}\0");
            if let Some(p) = unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(user32, cname.as_ptr()) } {
                orig.store(p as usize, Ordering::Relaxed);
            }
        }
    }
    // gdi32's screen size (Unity 5's player)
    for &base in &modules {
        if let Some(o) = unsafe { iat::hook_module(base, "gdi32.dll", "GetDeviceCaps", device_caps as *const () as usize) } {
            if o != device_caps as *const () as usize {
                ORIG_DEVICE_CAPS.store(o, Ordering::Relaxed);
            }
        }
    }
    if ORIG_DEVICE_CAPS.load(Ordering::Relaxed) == 0 {
        let gdi32 = unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryW(windows_sys::w!("gdi32.dll")) };
        if let Some(p) = unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(gdi32, c"GetDeviceCaps".as_ptr().cast()) } {
            ORIG_DEVICE_CAPS.store(p as usize, Ordering::Relaxed);
        }
    }
    subclass_existing();
    let (sw, sh) = screen();
    log!("desktop: the game sees a {w}x{h} desktop, fitted on the {sw}x{sh} screen ({} module(s))", modules.len());
}

/// A point of the game's desktop from a point of the real screen (the gun).
pub fn from_screen(x: f64, y: f64) -> (f64, f64) {
    let (s, ox, oy) = fit();
    ((x - ox) / s, (y - oy) / s)
}
