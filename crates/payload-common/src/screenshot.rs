//! Direct3D 9 and 8 shims. `d3d9!Direct3DCreate9(Ex)` / `d3d8!Direct3DCreate8` are hooked in
//! the game executable (import table, or `GetProcAddress` for games loading d3d9 at run time
//! such as DxLib's Direct3D 9Ex) and the device methods are wrapped (shared vtables):
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
//! * `WAL_D3D9_QUERY_FIX=1`: `IDirect3DQuery9::GetData` writes at most the requested size.
//!   KOF '98 UMFE / 2002 UM poll an event query into a 1-byte variable at the top of their
//!   stack frame; DXVK writes the whole 4-byte BOOL, overwriting the saved EBP (crash after the
//!   first frame).
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
const D3D_CREATE_DEVICE_EX: usize = 20;
const DEV_RESET: usize = 16;
const DEV_PRESENT: usize = 17;
const DEV_PRESENT_EX: usize = 121;
const DEV_CREATE_QUERY: usize = 118;
const QUERY_GET_DATA: usize = 7;
const DEV_RESET_EX: usize = 132;
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
static ORIG_CREATE9EX: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_DEVICE_EX: AtomicUsize = AtomicUsize::new(0);
static ORIG_PRESENT_EX: AtomicUsize = AtomicUsize::new(0);
static ORIG_RESET_EX: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_QUERY: AtomicUsize = AtomicUsize::new(0);
static ORIG_QUERY_GET_DATA: AtomicUsize = AtomicUsize::new(0);
static QUERY_FIX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
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
    let query_fix = std::env::var("WAL_D3D9_QUERY_FIX").is_ok_and(|v| v == "1");
    QUERY_FIX.store(query_fix, Ordering::Relaxed);
    if secs.is_none() && size.is_none() && !force && !query_fix {
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
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        ORIG_GET_PROC_ADDRESS.store(o, Ordering::Relaxed);
    }
}

/// Games loading d3d9/d3d8 at run time get the wrapped creation functions.
unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> usize {
    let orig: unsafe extern "system" fn(P, *const u8) -> usize =
        unsafe { std::mem::transmute(ORIG_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if f == 0 || (name as usize) >> 16 == 0 {
        return f; // not found, or by ordinal
    }
    let (slot, wrapper): (&AtomicUsize, usize) = match unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() {
        b"Direct3DCreate9" => (&ORIG_CREATE9, create9 as *const () as usize),
        b"Direct3DCreate9Ex" => (&ORIG_CREATE9EX, create9ex as *const () as usize),
        b"Direct3DCreate8" => (&d3d8::ORIG_CREATE8, d3d8::create8 as *const () as usize),
        _ => return f,
    };
    if slot.swap(f, Ordering::Relaxed) == 0 {
        log!("d3d: shims on {} (GetProcAddress)", unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy());
    }
    wrapper
}

unsafe extern "system" fn create9ex(sdk: u32, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(u32, *mut P) -> HRESULT = unsafe { std::mem::transmute(ORIG_CREATE9EX.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(sdk, out) };
    if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
        unsafe {
            patch(*out, D3D_CREATE_DEVICE, create_device as *const () as usize, &ORIG_CREATE_DEVICE);
            patch(*out, D3D_CREATE_DEVICE_EX, create_device_ex as *const () as usize, &ORIG_CREATE_DEVICE_EX);
        }
    }
    hr
}

/// Device methods wrapped on every created device (9 and 9Ex devices share their vtable in
/// DXVK and wined3d).
unsafe fn wrap_device(dev: P) {
    unsafe {
        patch(dev, DEV_PRESENT, present as *const () as usize, &ORIG_PRESENT);
        patch(dev, DEV_RESET, reset as *const () as usize, &ORIG_RESET);
        if QUERY_FIX.load(Ordering::Relaxed) {
            patch(dev, DEV_CREATE_QUERY, create_query as *const () as usize, &ORIG_CREATE_QUERY);
        }
    }
}

unsafe extern "system" fn create_query(dev: P, kind: u32, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, u32, *mut P) -> HRESULT =
        unsafe { std::mem::transmute(ORIG_CREATE_QUERY.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(dev, kind, out) };
    if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
        unsafe { patch(*out, QUERY_GET_DATA, query_get_data as *const () as usize, &ORIG_QUERY_GET_DATA) };
    }
    hr
}

/// Query results (BOOL, UINT64, structures) into a scratch buffer, `size` bytes copied back.
unsafe extern "system" fn query_get_data(query: P, data: *mut u8, size: u32, flags: u32) -> HRESULT {
    let orig: unsafe extern "system" fn(P, *mut u8, u32, u32) -> HRESULT =
        unsafe { std::mem::transmute(ORIG_QUERY_GET_DATA.load(Ordering::Relaxed)) };
    if data.is_null() || size == 0 || size >= 64 {
        return unsafe { orig(query, data, size, flags) };
    }
    let mut buf = [0u8; 64];
    let hr = unsafe { orig(query, buf.as_mut_ptr(), size, flags) };
    if hr == 0 {
        // S_OK: data available (S_FALSE leaves the caller's buffer untouched)
        unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), data, size as usize) };
    }
    hr
}

unsafe extern "system" fn create_device_ex(d3d: P, adapter: u32, kind: u32, window: P, flags: u32, params: P, mode: P, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, u32, u32, P, u32, P, P, *mut P) -> HRESULT =
        unsafe { std::mem::transmute(ORIG_CREATE_DEVICE_EX.load(Ordering::Relaxed)) };
    unsafe { override_size(params) };
    let mut fallback = DisplayModeEx::default();
    let mode = unsafe { fullscreen_mode(params, mode, &mut fallback) };
    let hr = unsafe { orig(d3d, adapter, kind, window, flags, params, mode, out) };
    if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
        unsafe {
            wrap_device(*out);
            patch(*out, DEV_PRESENT_EX, present_ex as *const () as usize, &ORIG_PRESENT_EX);
            patch(*out, DEV_RESET_EX, reset_ex as *const () as usize, &ORIG_RESET_EX);
        }
    } else {
        unsafe { log_failure(hr, params, flags) };
    }
    hr
}

unsafe extern "system" fn present_ex(dev: P, src: P, dst: P, window: P, dirty: P, flags: u32) -> HRESULT {
    unsafe { maybe_capture(dev) };
    let orig: unsafe extern "system" fn(P, P, P, P, P, u32) -> HRESULT = unsafe { std::mem::transmute(ORIG_PRESENT_EX.load(Ordering::Relaxed)) };
    unsafe { orig(dev, src, dst, window, dirty, flags) }
}

/// D3DDISPLAYMODEEX
#[repr(C)]
#[derive(Default)]
struct DisplayModeEx {
    size: u32,
    width: u32,
    height: u32,
    refresh: u32,
    format: u32,
    scanline_ordering: u32,
}

/// 9Ex fullscreen devices need a display mode: games made fullscreen by the shims pass none.
unsafe fn fullscreen_mode(params: P, mode: P, fallback: &mut DisplayModeEx) -> P {
    if params.is_null() || !mode.is_null() {
        return mode;
    }
    let p = params as *const u32;
    if unsafe { *p.add(8) } != 0 {
        return mode; // windowed
    }
    unsafe {
        // display modes have no alpha: A8R8G8B8 back buffers are shown in X8R8G8B8
        let format = if *p.add(2) == D3DFMT_A8R8G8B8 { D3DFMT_X8R8G8B8 } else { *p.add(2) };
        *fallback = DisplayModeEx { size: 24, width: *p, height: *p.add(1), refresh: *p.add(12), format, scanline_ordering: 1 };
    }
    fallback as *mut DisplayModeEx as P
}

unsafe extern "system" fn reset_ex(dev: P, params: P, mode: P) -> HRESULT {
    unsafe { override_size(params) };
    let mut fallback = DisplayModeEx::default();
    let mode = unsafe { fullscreen_mode(params, mode, &mut fallback) };
    let orig: unsafe extern "system" fn(P, P, P) -> HRESULT = unsafe { std::mem::transmute(ORIG_RESET_EX.load(Ordering::Relaxed)) };
    unsafe { orig(dev, params, mode) }
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
        unsafe { log_device("d3d9", params, 8, window) };
        unsafe { wrap_device(*out) };
    } else {
        unsafe { log_failure(hr, params, flags) };
    }
    hr
}

unsafe fn log_failure(hr: HRESULT, params: P, flags: u32) {
    if params.is_null() {
        return;
    }
    let p = params as *const u32;
    unsafe {
        log!(
            "d3d9: CreateDevice failed {hr:#x}: {}x{} format {} buffers {} multisample {} swap {} windowed {} depth {}/{} flags {:#x} refresh {} interval {:#x}, behavior {flags:#x}",
            *p, *p.add(1), *p.add(2), *p.add(3), *p.add(4), *p.add(6), *p.add(8), *p.add(9), *p.add(10), *p.add(11), *p.add(12), *p.add(13)
        );
    }
}

unsafe fn maybe_capture(dev: P) {
    if let Some(path) = screenshot_due() {
        if let Err(e) = unsafe { capture(dev, &path) } {
            log!("screenshot: {path}: {e}");
        }
    }
}

unsafe extern "system" fn present(dev: P, src: P, dst: P, window: P, dirty: P) -> HRESULT {
    unsafe { maybe_capture(dev) };
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
            if !params.is_null() {
                let p = params as *const u32;
                let device_window = unsafe { *p.add(6) } as usize as P;
                unsafe {
                    log!(
                        "d3d8: device {}x{} windowed {}, focus window {}, device window {}",
                        *p, *p.add(1), *p.add(WINDOWED), describe_window(window), describe_window(device_window)
                    );
                }
            }
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

/// A window's handle, parent, style, visibility and screen rectangle, for the device logs.
fn describe_window(hwnd: P) -> String {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GWL_STYLE, GetParent, GetWindowLongW, GetWindowRect, IsWindowVisible};
    if hwnd.is_null() {
        return "null".into();
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe {
        GetWindowRect(hwnd, &mut r);
        format!(
            "{hwnd:?} (parent {:?}, style {:#x} ex {:#x}, visible {}, {},{} {}x{})",
            GetParent(hwnd),
            GetWindowLongW(hwnd, GWL_STYLE),
            GetWindowLongW(hwnd, GWL_EXSTYLE),
            IsWindowVisible(hwnd),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top
        )
    }
}


/// Logs a created device: back buffer size, windowed, focus and device windows.
unsafe fn log_device(api: &str, params: P, windowed_index: usize, window: P) {
    if params.is_null() {
        return;
    }
    let p = params as *const u32;
    let device_window = unsafe { *p.add(windowed_index - 1) } as usize as P;
    unsafe {
        log!(
            "{api}: device {}x{} windowed {}, focus window {}, device window {}",
            *p, *p.add(1), *p.add(windowed_index), describe_window(window), describe_window(device_window)
        );
    }
}
