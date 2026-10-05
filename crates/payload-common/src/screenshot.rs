//! Direct3D 9 and 8 shims. `d3d9!Direct3DCreate9` / `d3d8!Direct3DCreate8` are hooked in the
//! game executable and the device methods are wrapped (shared vtables):
//!
//! * `WAL_SCREENSHOT=<seconds>`: periodic screenshots for automated tests: every `<seconds>`
//!   the back buffer is written as `shot-NNNN.bmp` in the working directory
//!   (`WAL_SCREENSHOT_DIR` overrides it).
//! * `WAL_D3D9_FULLSCREEN_SIZE=<w>x<h>`: fullscreen devices are created (and reset) with this
//!   back buffer size. For games asking a display mode Wine cannot emulate (Type X2 1280x768);
//!   Wine's fullscreen scaling then fits it to the monitor.
//! * `WAL_D3D9_FULLSCREEN=1`: windowed devices are created (and reset) fullscreen instead,
//!   with the same back buffer size, for games that only run in a window under Wine.
//!
//! Direct3D 8 differs in the vtable slots and structure layouts only (see `mod d3d8`).

use std::ffi::c_void;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::{iat, log};

type HRESULT = i32;
type P = *mut c_void;

const D3D_CREATE_DEVICE: usize = 16;
const DEV_RESET: usize = 16;
const DEV_PRESENT: usize = 17;
const DEV_GET_BACK_BUFFER: usize = 18;
const DEV_GET_RENDER_TARGET_DATA: usize = 32;
const DEV_CREATE_OFFSCREEN_PLAIN_SURFACE: usize = 36;
const SURF_GET_DESC: usize = 12;
const SURF_LOCK_RECT: usize = 13;
const SURF_UNLOCK_RECT: usize = 14;
const RELEASE: usize = 2;
const D3DPOOL_SYSTEMMEM: u32 = 2;
const D3DFMT_A8R8G8B8: u32 = 21;
const D3DFMT_X8R8G8B8: u32 = 22;
const D3DLOCK_READONLY: u32 = 0x10;

static ORIG_CREATE9: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static ORIG_PRESENT: AtomicUsize = AtomicUsize::new(0);
static ORIG_RESET: AtomicUsize = AtomicUsize::new(0);
static FULLSCREEN_SIZE: Mutex<Option<(u32, u32)>> = Mutex::new(None);
static FORCE_FULLSCREEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct State {
    interval: Duration,
    last: Option<Instant>,
    count: u32,
    dir: String,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

unsafe fn method(obj: P, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

unsafe fn release(obj: P) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, RELEASE)) };
        unsafe { f(obj) };
    }
}

unsafe fn patch(obj: P, slot: usize, replacement: usize, orig: &AtomicUsize) {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    if orig.load(Ordering::Relaxed) != 0 {
        return;
    }
    let entry = unsafe { (*(obj as *const *mut usize)).add(slot) };
    let mut old = 0;
    unsafe {
        VirtualProtect(entry.cast(), size_of::<usize>(), PAGE_READWRITE, &mut old);
        orig.store(*entry, Ordering::Relaxed);
        *entry = replacement;
        VirtualProtect(entry.cast(), size_of::<usize>(), old, &mut old);
    }
}

pub fn init() {
    let size = std::env::var("WAL_D3D9_FULLSCREEN_SIZE").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
    });
    *FULLSCREEN_SIZE.lock().unwrap() = size;
    let force = std::env::var("WAL_D3D9_FULLSCREEN").is_ok_and(|v| v == "1");
    FORCE_FULLSCREEN.store(force, Ordering::Relaxed);
    let secs = std::env::var("WAL_SCREENSHOT").ok().and_then(|v| v.parse::<f64>().ok());
    if let Some(secs) = secs {
        let dir = std::env::var("WAL_SCREENSHOT_DIR").unwrap_or_else(|_| ".".into());
        *STATE.lock().unwrap() = Some(State { interval: Duration::from_secs_f64(secs.max(0.5)), last: None, count: 0, dir });
    }
    if secs.is_none() && size.is_none() && !force {
        return;
    }
    if let Some(o) = unsafe { iat::hook("d3d9.dll", "Direct3DCreate9", create9 as *const () as usize) } {
        ORIG_CREATE9.store(o, Ordering::Relaxed);
        log!("d3d9: shims enabled (screenshot every {secs:?}s, fullscreen size {size:?}, force fullscreen {force})");
    }
    if let Some(o) = unsafe { iat::hook("d3d8.dll", "Direct3DCreate8", d3d8::create8 as *const () as usize) } {
        d3d8::ORIG_CREATE8.store(o, Ordering::Relaxed);
        log!("d3d8: shims enabled (screenshot every {secs:?}s, fullscreen size {size:?}, force fullscreen {force})");
    }
}

/// Next screenshot path when one is due.
fn screenshot_due() -> Option<String> {
    let mut state = STATE.lock().unwrap();
    match state.as_mut() {
        Some(s) if s.last.is_none_or(|t| t.elapsed() >= s.interval) => {
            s.last = Some(Instant::now());
            s.count += 1;
            Some(format!("{}\\shot-{:04}.bmp", s.dir, s.count))
        }
        _ => None,
    }
}

/// D3DPRESENT_PARAMETERS: back buffer size at +0/+4, Windowed at +8 dwords... (x86 layout:
/// Width, Height, Format, Count, MultiSample, MultiSampleQuality, SwapEffect, hDeviceWindow,
/// Windowed, ...). Direct3D 8 has no MultiSampleQuality: Windowed is dword 7.
unsafe fn override_size(params: P) {
    unsafe { override_size_at(params, 8) }
}

/// Format is dword 2 and FullScreen_RefreshRateInHz is 4 dwords after Windowed in both layouts.
unsafe fn override_size_at(params: P, windowed_index: usize) {
    if params.is_null() {
        return;
    }
    let p = params as *mut u32;
    if FORCE_FULLSCREEN.load(Ordering::Relaxed) && unsafe { *p.add(windowed_index) } != 0 {
        let (w, h) = unsafe { (*p, *p.add(1)) };
        if w != 0 && h != 0 {
            unsafe {
                *p.add(windowed_index) = 0;
                if *p.add(2) == 0 {
                    *p.add(2) = D3DFMT_X8R8G8B8; // D3DFMT_UNKNOWN is windowed only
                }
                if *p.add(windowed_index + 4) == 0 {
                    *p.add(windowed_index + 4) = 60;
                }
            }
            log!("d3d: windowed {w}x{h} device made fullscreen");
        }
    }
    let Some((w, h)) = *FULLSCREEN_SIZE.lock().unwrap() else { return };
    let windowed = unsafe { *p.add(windowed_index) } != 0;
    if !windowed {
        let (old_w, old_h) = unsafe { (*p, *p.add(1)) };
        unsafe {
            *p = w;
            *p.add(1) = h;
        }
        log!("d3d9: fullscreen back buffer {old_w}x{old_h} -> {w}x{h}");
    }
}

unsafe extern "system" fn reset(dev: P, params: P) -> HRESULT {
    unsafe { override_size(params) };
    let orig: unsafe extern "system" fn(P, P) -> HRESULT = unsafe { std::mem::transmute(ORIG_RESET.load(Ordering::Relaxed)) };
    unsafe { orig(dev, params) }
}

unsafe extern "system" fn create9(sdk: u32) -> P {
    let orig: unsafe extern "system" fn(u32) -> P = unsafe { std::mem::transmute(ORIG_CREATE9.load(Ordering::Relaxed)) };
    let d3d = unsafe { orig(sdk) };
    if !d3d.is_null() {
        unsafe { patch(d3d, D3D_CREATE_DEVICE, create_device as *const () as usize, &ORIG_CREATE_DEVICE) };
    }
    d3d
}

unsafe extern "system" fn create_device(d3d: P, adapter: u32, kind: u32, window: P, flags: u32, params: P, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, u32, u32, P, u32, P, *mut P) -> HRESULT =
        unsafe { std::mem::transmute(ORIG_CREATE_DEVICE.load(Ordering::Relaxed)) };
    unsafe { override_size(params) };
    let hr = unsafe { orig(d3d, adapter, kind, window, flags, params, out) };
    if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
        unsafe {
            patch(*out, DEV_PRESENT, present as *const () as usize, &ORIG_PRESENT);
            patch(*out, DEV_RESET, reset as *const () as usize, &ORIG_RESET);
        }
    } else if !params.is_null() {
        let p = params as *const u32;
        unsafe {
            log!(
                "d3d9: CreateDevice failed {hr:#x}: {}x{} format {} buffers {} multisample {} swap {} windowed {} depth {}/{} flags {:#x} refresh {} interval {:#x}, behavior {flags:#x}",
                *p, *p.add(1), *p.add(2), *p.add(3), *p.add(4), *p.add(6), *p.add(8), *p.add(9), *p.add(10), *p.add(11), *p.add(12), *p.add(13)
            );
        }
    }
    hr
}

unsafe extern "system" fn present(dev: P, src: P, dst: P, window: P, dirty: P) -> HRESULT {
    if let Some(path) = screenshot_due() {
        if let Err(e) = unsafe { capture(dev, &path) } {
            log!("screenshot: {path}: {e}");
        }
    }
    let orig: unsafe extern "system" fn(P, P, P, P, P) -> HRESULT = unsafe { std::mem::transmute(ORIG_PRESENT.load(Ordering::Relaxed)) };
    unsafe { orig(dev, src, dst, window, dirty) }
}

/// D3DSURFACE_DESC
#[repr(C)]
#[derive(Default)]
struct SurfaceDesc {
    format: u32,
    kind: u32,
    usage: u32,
    pool: u32,
    multisample: u32,
    multisample_quality: u32,
    width: u32,
    height: u32,
}

unsafe fn capture(dev: P, path: &str) -> Result<(), String> {
    unsafe {
        let get_back_buffer: unsafe extern "system" fn(P, u32, u32, u32, *mut P) -> HRESULT =
            std::mem::transmute(method(dev, DEV_GET_BACK_BUFFER));
        let mut back: P = std::ptr::null_mut();
        if get_back_buffer(dev, 0, 0, 0, &mut back) < 0 || back.is_null() {
            return Err("GetBackBuffer failed".into());
        }
        let get_desc: unsafe extern "system" fn(P, *mut SurfaceDesc) -> HRESULT = std::mem::transmute(method(back, SURF_GET_DESC));
        let mut desc = SurfaceDesc::default();
        get_desc(back, &mut desc);
        if desc.format != D3DFMT_X8R8G8B8 && desc.format != D3DFMT_A8R8G8B8 {
            release(back);
            return Err(format!("unsupported back buffer format {}", desc.format));
        }
        let create: unsafe extern "system" fn(P, u32, u32, u32, u32, *mut P, P) -> HRESULT =
            std::mem::transmute(method(dev, DEV_CREATE_OFFSCREEN_PLAIN_SURFACE));
        let mut sys: P = std::ptr::null_mut();
        if create(dev, desc.width, desc.height, desc.format, D3DPOOL_SYSTEMMEM, &mut sys, std::ptr::null_mut()) < 0 {
            release(back);
            return Err("CreateOffscreenPlainSurface failed".into());
        }
        let get_data: unsafe extern "system" fn(P, P, P) -> HRESULT = std::mem::transmute(method(dev, DEV_GET_RENDER_TARGET_DATA));
        let hr = get_data(dev, back, sys);
        release(back);
        if hr < 0 {
            release(sys);
            return Err(format!("GetRenderTargetData failed {hr:#x}"));
        }
        #[repr(C)]
        struct Locked {
            pitch: i32,
            bits: *const u8,
        }
        let mut locked = Locked { pitch: 0, bits: std::ptr::null() };
        let lock: unsafe extern "system" fn(P, *mut Locked, P, u32) -> HRESULT = std::mem::transmute(method(sys, SURF_LOCK_RECT));
        if lock(sys, &mut locked, std::ptr::null_mut(), D3DLOCK_READONLY) < 0 {
            release(sys);
            return Err("LockRect failed".into());
        }
        let pixels = read_pixels(locked.bits, locked.pitch, desc.width, desc.height);
        let unlock: unsafe extern "system" fn(P) -> HRESULT = std::mem::transmute(method(sys, SURF_UNLOCK_RECT));
        unlock(sys);
        release(sys);
        save(path, desc.width, desc.height, &pixels)
    }
}

/// Copies a locked 32-bit surface.
unsafe fn read_pixels(bits: *const u8, pitch: i32, w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = unsafe { std::slice::from_raw_parts(bits.offset(y as isize * pitch as isize), w * 4) };
        pixels.extend_from_slice(row);
    }
    pixels
}

fn save(path: &str, w: u32, h: u32, pixels: &[u8]) -> Result<(), String> {
    write_bmp(path, w, h, pixels).map_err(|e| e.to_string())?;
    log!("screenshot: {path} ({w}x{h})");
    Ok(())
}

/// 32-bit top-down BMP (BGRX rows, as in X8R8G8B8 surfaces).
fn write_bmp(path: &str, w: u32, h: u32, bgrx: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    let size = 54 + bgrx.len() as u32;
    f.write_all(b"BM")?;
    f.write_all(&size.to_le_bytes())?;
    f.write_all(&0u32.to_le_bytes())?;
    f.write_all(&54u32.to_le_bytes())?;
    f.write_all(&40u32.to_le_bytes())?;
    f.write_all(&(w as i32).to_le_bytes())?;
    f.write_all(&(-(h as i32)).to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(&[0u8; 24])?;
    f.write_all(bgrx)
}

/// Direct3D 8: same shims. Slots: IDirect3D8::CreateDevice 15; IDirect3DDevice8 Reset 14,
/// Present 15, GetBackBuffer 16, CreateImageSurface 27, CopyRects 28; IDirect3DSurface8
/// GetDesc 8, LockRect 9, UnlockRect 10.
mod d3d8 {
    use super::*;

    const D3D_CREATE_DEVICE: usize = 15;
    const DEV_RESET: usize = 14;
    const DEV_PRESENT: usize = 15;
    const DEV_GET_BACK_BUFFER: usize = 16;
    const DEV_CREATE_IMAGE_SURFACE: usize = 27;
    const DEV_COPY_RECTS: usize = 28;
    const SURF_GET_DESC: usize = 8;
    const SURF_LOCK_RECT: usize = 9;
    const SURF_UNLOCK_RECT: usize = 10;
    /// D3DPRESENT_PARAMETERS (8): Width, Height, Format, Count, MultiSample, SwapEffect,
    /// hDeviceWindow, Windowed, ...
    const WINDOWED: usize = 7;

    pub static ORIG_CREATE8: AtomicUsize = AtomicUsize::new(0);
    static ORIG_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
    static ORIG_PRESENT: AtomicUsize = AtomicUsize::new(0);
    static ORIG_RESET: AtomicUsize = AtomicUsize::new(0);

    pub unsafe extern "system" fn create8(sdk: u32) -> P {
        let orig: unsafe extern "system" fn(u32) -> P = unsafe { std::mem::transmute(ORIG_CREATE8.load(Ordering::Relaxed)) };
        let d3d = unsafe { orig(sdk) };
        if !d3d.is_null() {
            unsafe { patch(d3d, D3D_CREATE_DEVICE, create_device as *const () as usize, &ORIG_CREATE_DEVICE) };
        }
        d3d
    }

    unsafe extern "system" fn create_device(d3d: P, adapter: u32, kind: u32, window: P, flags: u32, params: P, out: *mut P) -> HRESULT {
        let orig: unsafe extern "system" fn(P, u32, u32, P, u32, P, *mut P) -> HRESULT =
            unsafe { std::mem::transmute(ORIG_CREATE_DEVICE.load(Ordering::Relaxed)) };
        unsafe { override_size_at(params, WINDOWED) };
        let hr = unsafe { orig(d3d, adapter, kind, window, flags, params, out) };
        if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
            unsafe {
                patch(*out, DEV_PRESENT, present as *const () as usize, &ORIG_PRESENT);
                patch(*out, DEV_RESET, reset as *const () as usize, &ORIG_RESET);
            }
        } else if !params.is_null() {
            let p = params as *const u32;
            unsafe {
                log!(
                    "d3d8: CreateDevice failed {hr:#x}: {}x{} format {} buffers {} multisample {} swap {} windowed {} refresh {} interval {:#x}, behavior {flags:#x}",
                    *p, *p.add(1), *p.add(2), *p.add(3), *p.add(4), *p.add(5), *p.add(7), *p.add(11), *p.add(12)
                );
            }
        }
        hr
    }

    unsafe extern "system" fn reset(dev: P, params: P) -> HRESULT {
        unsafe { override_size_at(params, WINDOWED) };
        let orig: unsafe extern "system" fn(P, P) -> HRESULT = unsafe { std::mem::transmute(ORIG_RESET.load(Ordering::Relaxed)) };
        unsafe { orig(dev, params) }
    }

    unsafe extern "system" fn present(dev: P, src: P, dst: P, window: P, dirty: P) -> HRESULT {
        if let Some(path) = screenshot_due() {
            if let Err(e) = unsafe { capture(dev, &path) } {
                log!("screenshot: {path}: {e}");
            }
        }
        let orig: unsafe extern "system" fn(P, P, P, P, P) -> HRESULT = unsafe { std::mem::transmute(ORIG_PRESENT.load(Ordering::Relaxed)) };
        unsafe { orig(dev, src, dst, window, dirty) }
    }

    /// D3DSURFACE_DESC (8)
    #[repr(C)]
    #[derive(Default)]
    struct SurfaceDesc {
        format: u32,
        kind: u32,
        usage: u32,
        pool: u32,
        size: u32,
        multisample: u32,
        width: u32,
        height: u32,
    }

    unsafe fn capture(dev: P, path: &str) -> Result<(), String> {
        unsafe {
            let get_back_buffer: unsafe extern "system" fn(P, u32, u32, *mut P) -> HRESULT =
                std::mem::transmute(method(dev, DEV_GET_BACK_BUFFER));
            let mut back: P = std::ptr::null_mut();
            if get_back_buffer(dev, 0, 0, &mut back) < 0 || back.is_null() {
                return Err("GetBackBuffer failed".into());
            }
            let get_desc: unsafe extern "system" fn(P, *mut SurfaceDesc) -> HRESULT = std::mem::transmute(method(back, SURF_GET_DESC));
            let mut desc = SurfaceDesc::default();
            get_desc(back, &mut desc);
            if desc.format != D3DFMT_X8R8G8B8 && desc.format != D3DFMT_A8R8G8B8 {
                release(back);
                return Err(format!("unsupported back buffer format {}", desc.format));
            }
            let create: unsafe extern "system" fn(P, u32, u32, u32, *mut P) -> HRESULT =
                std::mem::transmute(method(dev, DEV_CREATE_IMAGE_SURFACE));
            let mut sys: P = std::ptr::null_mut();
            if create(dev, desc.width, desc.height, desc.format, &mut sys) < 0 {
                release(back);
                return Err("CreateImageSurface failed".into());
            }
            let copy: unsafe extern "system" fn(P, P, P, u32, P, P) -> HRESULT = std::mem::transmute(method(dev, DEV_COPY_RECTS));
            let hr = copy(dev, back, std::ptr::null_mut(), 0, sys, std::ptr::null_mut());
            release(back);
            if hr < 0 {
                release(sys);
                return Err(format!("CopyRects failed {hr:#x}"));
            }
            #[repr(C)]
            struct Locked {
                pitch: i32,
                bits: *const u8,
            }
            let mut locked = Locked { pitch: 0, bits: std::ptr::null() };
            let lock: unsafe extern "system" fn(P, *mut Locked, P, u32) -> HRESULT = std::mem::transmute(method(sys, SURF_LOCK_RECT));
            if lock(sys, &mut locked, std::ptr::null_mut(), D3DLOCK_READONLY) < 0 {
                release(sys);
                return Err("LockRect failed".into());
            }
            let pixels = read_pixels(locked.bits, locked.pitch, desc.width, desc.height);
            let unlock: unsafe extern "system" fn(P) -> HRESULT = std::mem::transmute(method(sys, SURF_UNLOCK_RECT));
            unlock(sys);
            release(sys);
            save(path, desc.width, desc.height, &pixels)
        }
    }
}
