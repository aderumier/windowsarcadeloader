//! Two 640x480 monitors in one window (Wartran Troopers, Lethal Enforcers 3).
//!
//! The cabinets have two 640x480 monitors side by side: the games create one Direct3D 8 device
//! per adapter, each full screen in its own window (at x 0 and 640), when `GetAdapterCount`
//! reports two adapters; with one they play on the left monitor alone. Here the second adapter
//! is the first one again, both devices are windowed (copy swap, one back buffer) on the first
//! screen's window (the second one hidden) and each presents into its half of the 1280x480
//! picture, fitted in the window with its aspect (`WAL_KEEP_ASPECT_RATIO=0`: stretched to the
//! whole window). The guns aim over the whole picture (`picture_area`). `init_first_only`: the
//! game still gets both screens, only the first one is shown (Lethal Enforcers 3's 1 player
//! mode: the game itself has no working one-monitor mode).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, SW_HIDE, ShowWindow};

type P = *mut c_void;
type HRESULT = i32;

const SCREEN_W: i32 = 640;
const SCREEN_H: i32 = 480;
/// IDirect3D8 methods taking an adapter (GetAdapterIdentifier .. GetAdapterMonitor).
const GET_ADAPTER_COUNT: usize = 4;
const CREATE_DEVICE: usize = 15;
const DEV_PRESENT: usize = 15;
/// D3DPRESENT_PARAMETERS (8) fields.
const PP_FORMAT: usize = 2;
const PP_BACK_BUFFERS: usize = 3;
const PP_SWAP_EFFECT: usize = 5;
const PP_DEVICE_WINDOW: usize = 6;
const PP_WINDOWED: usize = 7;
const PP_REFRESH: usize = 11;
const PP_INTERVAL: usize = 12;
const D3DFMT_X8R8G8B8: u32 = 22;
const D3DSWAPEFFECT_COPY: u32 = 3;

static ORIG_CREATE8: AtomicUsize = AtomicUsize::new(0);
static ORIG: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
static ORIG_PRESENT: AtomicUsize = AtomicUsize::new(0);
/// The devices of adapters 0 and 1, and the window both present to.
static DEVICES: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
static MAIN_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// The picture's area in the window, in 1/65536 of its size: left, top, width, height.
static AREA: [AtomicU32; 4] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(65536), AtomicU32::new(65536)];

/// The game's name in the log.
static GAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
/// The picture keeps its aspect (black bars), else it is stretched to the window.
static KEEP_ASPECT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Only the first screen shown (over the whole picture).
static FIRST_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// As `init`, only the first screen shown.
pub fn init_first_only(game: &'static str) {
    FIRST_ONLY.store(true, Ordering::Relaxed);
    init(game);
}

fn game() -> &'static str {
    GAME.get().copied().unwrap_or("konami")
}

pub fn init(game: &'static str) {
    let _ = GAME.set(game);
    KEEP_ASPECT.store(std::env::var("WAL_KEEP_ASPECT_RATIO").map_or(true, |v| v.trim() != "0"), Ordering::Relaxed);
    match unsafe { iat::hook("d3d8.dll", "Direct3DCreate8", create8 as *const () as usize) } {
        Some(o) => ORIG_CREATE8.store(o, Ordering::Relaxed),
        None => log!("{game}: Direct3DCreate8 import not found, one screen"),
    }
}

/// The picture's area in the window (left, top, width, height, 0..1).
pub fn picture_area() -> (f32, f32, f32, f32) {
    let f = |i: usize| AREA[i].load(Ordering::Relaxed) as f32 / 65536.0;
    (f(0), f(1), f(2), f(3))
}

unsafe fn patch(obj: P, index: usize, f: usize, orig: &AtomicUsize) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    unsafe {
        let slot = (*(obj as *const *mut usize)).add(index);
        if *slot == f {
            return;
        }
        let mut old = 0;
        VirtualProtect(slot.cast(), 4, PAGE_EXECUTE_READWRITE, &mut old);
        orig.store(*slot, Ordering::Relaxed);
        *slot = f;
        VirtualProtect(slot.cast(), 4, old, &mut old);
    }
}

unsafe extern "system" fn create8(sdk: u32) -> P {
    let orig: unsafe extern "system" fn(u32) -> P = unsafe { std::mem::transmute(ORIG_CREATE8.load(Ordering::Relaxed)) };
    let d3d = unsafe { orig(sdk) };
    if !d3d.is_null() {
        unsafe {
            patch(d3d, GET_ADAPTER_COUNT, get_adapter_count as *const () as usize, &ORIG[GET_ADAPTER_COUNT]);
            patch(d3d, 5, adapter3::<5> as *const () as usize, &ORIG[5]);
            patch(d3d, 6, adapter1::<6> as *const () as usize, &ORIG[6]);
            patch(d3d, 7, adapter3::<7> as *const () as usize, &ORIG[7]);
            patch(d3d, 8, adapter2::<8> as *const () as usize, &ORIG[8]);
            patch(d3d, 9, adapter5::<9> as *const () as usize, &ORIG[9]);
            patch(d3d, 10, adapter6::<10> as *const () as usize, &ORIG[10]);
            patch(d3d, 11, adapter5::<11> as *const () as usize, &ORIG[11]);
            patch(d3d, 12, adapter5::<12> as *const () as usize, &ORIG[12]);
            patch(d3d, 13, adapter3::<13> as *const () as usize, &ORIG[13]);
            patch(d3d, 14, adapter1::<14> as *const () as usize, &ORIG[14]);
            patch(d3d, CREATE_DEVICE, create_device as *const () as usize, &ORIG[CREATE_DEVICE]);
        }
        log!("{}: two screens (adapter 1 is adapter 0, both in one window)", game());
    }
    d3d
}

unsafe extern "system" fn get_adapter_count(_d3d: P) -> u32 {
    2
}

// The adapter queries, adapter 1 answered by adapter 0 (by argument count after it).
unsafe extern "system" fn adapter1<const I: usize>(d3d: P, _adapter: u32) -> usize {
    let f: unsafe extern "system" fn(P, u32) -> usize = unsafe { std::mem::transmute(ORIG[I].load(Ordering::Relaxed)) };
    unsafe { f(d3d, 0) }
}
unsafe extern "system" fn adapter2<const I: usize>(d3d: P, _adapter: u32, a: usize) -> usize {
    let f: unsafe extern "system" fn(P, u32, usize) -> usize = unsafe { std::mem::transmute(ORIG[I].load(Ordering::Relaxed)) };
    unsafe { f(d3d, 0, a) }
}
unsafe extern "system" fn adapter3<const I: usize>(d3d: P, _adapter: u32, a: usize, b: usize) -> usize {
    let f: unsafe extern "system" fn(P, u32, usize, usize) -> usize = unsafe { std::mem::transmute(ORIG[I].load(Ordering::Relaxed)) };
    unsafe { f(d3d, 0, a, b) }
}
unsafe extern "system" fn adapter5<const I: usize>(d3d: P, _adapter: u32, a: usize, b: usize, c: usize, d: usize) -> usize {
    let f: unsafe extern "system" fn(P, u32, usize, usize, usize, usize) -> usize = unsafe { std::mem::transmute(ORIG[I].load(Ordering::Relaxed)) };
    unsafe { f(d3d, 0, a, b, c, d) }
}
unsafe extern "system" fn adapter6<const I: usize>(d3d: P, _adapter: u32, a: usize, b: usize, c: usize, d: usize, e: usize) -> usize {
    let f: unsafe extern "system" fn(P, u32, usize, usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(ORIG[I].load(Ordering::Relaxed)) };
    unsafe { f(d3d, 0, a, b, c, d, e) }
}

unsafe extern "system" fn create_device(d3d: P, adapter: u32, kind: u32, window: P, flags: u32, params: P, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, u32, u32, P, u32, P, *mut P) -> HRESULT =
        unsafe { std::mem::transmute(ORIG[CREATE_DEVICE].load(Ordering::Relaxed)) };
    let index = (adapter as usize).min(1);
    // the screen's own window (the device window; the focus window when none)
    let screen_window = match params.is_null() {
        true => window,
        false => match unsafe { *(params as *const u32).add(PP_DEVICE_WINDOW) } {
            0 => window,
            w => w as usize as P,
        },
    };
    if index == 0 {
        MAIN_WINDOW.store(screen_window as usize, Ordering::Relaxed);
    } else if !screen_window.is_null() && screen_window as usize != MAIN_WINDOW.load(Ordering::Relaxed) {
        unsafe { ShowWindow(screen_window, SW_HIDE) };
    }
    let main = MAIN_WINDOW.load(Ordering::Relaxed) as P;
    let main = if main.is_null() { screen_window } else { main };
    if !params.is_null() {
        let p = params as *mut u32;
        unsafe {
            *p.add(PP_FORMAT) = D3DFMT_X8R8G8B8;
            // a copy swap (presented into a part of the window) has one back buffer
            *p.add(PP_BACK_BUFFERS) = 1;
            *p.add(PP_SWAP_EFFECT) = D3DSWAPEFFECT_COPY;
            *p.add(PP_DEVICE_WINDOW) = main as usize as u32;
            *p.add(PP_WINDOWED) = 1;
            *p.add(PP_REFRESH) = 0;
            *p.add(PP_INTERVAL) = 0;
        }
    }
    let hr = unsafe { orig(d3d, 0, kind, window, flags, params, out) };
    if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
        let dev = unsafe { *out };
        DEVICES[index].store(dev as usize, Ordering::Relaxed);
        unsafe { patch(dev, DEV_PRESENT, present as *const () as usize, &ORIG_PRESENT) };
        log!("{}: screen {} device created (windowed, its half of the window)", game(), index + 1);
    } else {
        log!("{}: screen {} CreateDevice failed {hr:#x}", game(), index + 1);
    }
    hr
}

/// Each device presents into its half of the picture, the picture fitted in the window.
unsafe extern "system" fn present(dev: P, src: P, _dst: P, _window: P, dirty: P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, P, P, P, P) -> HRESULT = unsafe { std::mem::transmute(ORIG_PRESENT.load(Ordering::Relaxed)) };
    let main = MAIN_WINDOW.load(Ordering::Relaxed) as P;
    let index = (DEVICES[1].load(Ordering::Relaxed) == dev as usize) as i32;
    let first_only = FIRST_ONLY.load(Ordering::Relaxed);
    if first_only && index == 1 {
        return 0;
    }
    // the screens shown side by side
    let shown = if first_only { 1 } else { 2 };
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if main.is_null() || unsafe { GetClientRect(main, &mut client) } == 0 || client.right <= 0 || client.bottom <= 0 {
        return unsafe { orig(dev, src, std::ptr::null_mut(), std::ptr::null_mut(), dirty) };
    }
    let (cw, ch) = (client.right as f32, client.bottom as f32);
    let (pw, ph) = if KEEP_ASPECT.load(Ordering::Relaxed) {
        let scale = (cw / (shown * SCREEN_W) as f32).min(ch / SCREEN_H as f32);
        ((shown * SCREEN_W) as f32 * scale, SCREEN_H as f32 * scale)
    } else {
        (cw, ch)
    };
    let (px, py) = ((cw - pw) / 2.0, (ch - ph) / 2.0);
    AREA[0].store((px / cw * 65536.0) as u32, Ordering::Relaxed);
    AREA[1].store((py / ch * 65536.0) as u32, Ordering::Relaxed);
    AREA[2].store((pw / cw * 65536.0) as u32, Ordering::Relaxed);
    AREA[3].store((ph / ch * 65536.0) as u32, Ordering::Relaxed);
    let half = pw / shown as f32;
    let mut dst = RECT {
        left: (px + half * index as f32) as i32,
        top: py as i32,
        right: (px + half * (index + 1) as f32) as i32,
        bottom: (py + ph) as i32,
    };
    unsafe { orig(dev, src, (&mut dst as *mut RECT).cast(), main, dirty) }
}
