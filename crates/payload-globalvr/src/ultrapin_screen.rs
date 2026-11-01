//! UltraPin's table picture: turned upright, the DMD composed into it.
//!
//! The cabinet's playfield monitor is turned on its side: `VPinball.exe` draws the table
//! sideways in its 1360x768 window (the table's top on the left). `WAL_ULTRAPIN_ROTATE` (on by
//! default, `0` off) shows it upright instead, filling the screen: the window is made the
//! screen's size, and each rectangle the player copies from its back buffer to the screen is
//! turned 90 degrees clockwise and scaled into the screen's height (centered, black on the
//! sides) in a surface of the payload's, copied to the screen in its place.
//!
//! VPinMAME (in `VPinball.exe`) shows the DMD in a window of its own (`MAME`), placed by the
//! frontend on the backglass monitor. Its frames are captured where VPinMAME paints them
//! (`StretchDIBits`, its GDI output: `ddraw` off) and drawn into the table's picture: upright in
//! the top left corner of the window (in the margin left of the table, when there is room), or,
//! not rotated, in the window's top left corner, half its height.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetSystemMetrics, GetWindowLongW, GetWindowThreadProcessId, HWND_TOPMOST,
    IsWindowVisible, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetWindowPos, ShowWindowAsync, WS_EX_TOPMOST,
};
use windows_sys::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, BITMAPINFO, BITMAPINFOHEADER, HALFTONE, HDC, SRCCOPY, SetStretchBltMode};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The last DMD frame: a top-down 0x00RRGGBB picture.
struct Frame {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}

static FRAME: Mutex<Option<Frame>> = Mutex::new(None);
/// The DMD frames captured.
static FRAMES: AtomicUsize = AtomicUsize::new(0);

static ORIG_STRETCH: AtomicUsize = AtomicUsize::new(0);

/// VPinMAME's `StretchDIBits` hooked once COM loads it.
pub(crate) fn init() {
    std::thread::spawn(|| {
        let name: Vec<u16> = "VPinMAME.dll\0".encode_utf16().collect();
        loop {
            let base = unsafe { GetModuleHandleW(name.as_ptr()) } as usize;
            if base != 0 {
                if let Some(o) = unsafe { iat::hook_module(base, "gdi32.dll", "StretchDIBits", stretch_di_bits as *const () as usize) } {
                    ORIG_STRETCH.store(o, Ordering::Relaxed);
                    log!("ultrapin: VPinMAME's DMD captured");
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

type StretchDIBits =
    unsafe extern "system" fn(HDC, i32, i32, i32, i32, i32, i32, i32, i32, *const u8, *const BITMAPINFO, u32, u32) -> i32;

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn stretch_di_bits(
    hdc: HDC,
    xd: i32,
    yd: i32,
    wd: i32,
    hd: i32,
    xs: i32,
    ys: i32,
    ws: i32,
    hs: i32,
    bits: *const u8,
    info: *const BITMAPINFO,
    usage: u32,
    rop: u32,
) -> i32 {
    unsafe { capture(xs, ys, ws, hs, bits, info, usage) };
    let orig: StretchDIBits = unsafe { std::mem::transmute(ORIG_STRETCH.load(Ordering::Relaxed)) };
    unsafe { orig(hdc, xd, yd, wd, hd, xs, ys, ws, hs, bits, info, usage, rop) }
}

/// The source rectangle of a DIB copied as a top-down 32-bit picture.
unsafe fn capture(xs: i32, ys: i32, ws: i32, hs: i32, bits: *const u8, info: *const BITMAPINFO, usage: u32) {
    if bits.is_null() || info.is_null() || ws <= 0 || hs <= 0 || usage != 0 {
        return;
    }
    let header: &BITMAPINFOHEADER = unsafe { &(*info).bmiHeader };
    let (bw, bh, bpp) = (header.biWidth, header.biHeight, header.biBitCount as i32);
    let top_down = bh < 0;
    let bh = bh.abs();
    if !matches!(bpp, 8 | 16 | 24 | 32) || header.biCompression != 0 && !(header.biCompression == 3 && bpp != 8) {
        log_once(&format!("ultrapin: DMD format {bpp} bpp, compression {} not captured", header.biCompression));
        return;
    }
    let stride = ((bw * bpp + 31) / 32 * 4) as usize;
    let palette = unsafe { (*info).bmiColors.as_ptr() };
    // BI_BITFIELDS: the masks after the header
    let masks = if header.biCompression == 3 { unsafe { std::slice::from_raw_parts(palette as *const u32, 3) } } else { &[0x7c00, 0x3e0, 0x1f][..] };
    let (w, h) = (ws.min(bw - xs) as usize, hs.min(bh - ys) as usize);
    if xs < 0 || ys < 0 || w == 0 || h == 0 {
        return;
    }
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        // a bottom-up DIB: its first line is the bottom one, the source y counts from the bottom
        let row = if top_down { ys as usize + y } else { ys as usize + h - 1 - y };
        let line = unsafe { bits.add(row * stride) };
        for x in xs as usize..xs as usize + w {
            let p = unsafe {
                match bpp {
                    8 => {
                        let q = *palette.add(*line.add(x) as usize);
                        u32::from_le_bytes([q.rgbBlue, q.rgbGreen, q.rgbRed, 0])
                    }
                    16 => rgb16(u16::from_le_bytes([*line.add(2 * x), *line.add(2 * x + 1)]) as u32, masks),
                    24 => u32::from_le_bytes([*line.add(3 * x), *line.add(3 * x + 1), *line.add(3 * x + 2), 0]),
                    _ => (line.add(4 * x) as *const u32).read_unaligned() & 0xff_ffff,
                }
            };
            pixels.push(p);
        }
    }
    log_once(&format!("ultrapin: DMD {bw}x{bh} {bpp} bpp, source {xs},{ys} {ws}x{hs}"));
    *FRAME.lock().unwrap_or_else(|e| e.into_inner()) = Some(Frame { width: w, height: h, pixels });
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// A 16-bit pixel of the given masks as 0x00RRGGBB.
fn rgb16(p: u32, masks: &[u32]) -> u32 {
    let channel = |mask: u32| {
        if mask == 0 {
            return 0;
        }
        let v = (p & mask) >> mask.trailing_zeros();
        let max = mask >> mask.trailing_zeros();
        v * 255 / max
    };
    channel(masks[0]) << 16 | channel(masks[1]) << 8 | channel(masks[2])
}

fn log_once(message: &str) {
    static LAST: Mutex<String> = Mutex::new(String::new());
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last != message {
        log!("{message}");
        *last = message.to_string();
    }
}

// ---- the table's picture: VPinball's DirectDraw primary surface ----
//
// The player renders into a back buffer and copies its changed rectangles to the primary
// surface (`Blt`, the window's screen rectangle). Each copy touching the DMD's rectangle (at
// the top left, half the picture's height) gets the DMD drawn into the back buffer first; a new
// DMD frame is copied to the screen with the next copy.

type P = *mut c_void;

const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
/// `IDirectDraw7::CreateSurface`.
const CREATE_SURFACE: usize = 6;
/// `IDirectDrawSurface7::Blt` / `GetDC` / `GetSurfaceDesc` / `ReleaseDC`.
const BLT: usize = 5;
const GET_DC: usize = 17;
const GET_SURFACE_DESC: usize = 22;
const LOCK: usize = 25;
const RELEASE_DC: usize = 26;
const UNLOCK: usize = 32;
const DDLOCK_WAIT: u32 = 0x1;
const DDLOCK_READONLY: u32 = 0x10;
const DDLOCK_WRITEONLY: u32 = 0x20;
const DDBLT_WAIT: u32 = 0x100_0000;
const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSCAPS_OFFSCREENPLAIN: u32 = 0x40;
const DDSCAPS_SYSTEMMEMORY: u32 = 0x800;

static ORIG_CREATE_DD: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_SURFACE: AtomicUsize = AtomicUsize::new(0);
static ORIG_BLT: AtomicUsize = AtomicUsize::new(0);
static PRIMARY: AtomicUsize = AtomicUsize::new(0);
/// The player's `IDirectDraw7`.
static DDRAW: AtomicUsize = AtomicUsize::new(0);
/// The DMD frame last copied to the screen.
static SHOWN: AtomicUsize = AtomicUsize::new(0);

static ORIG_GET_PROC: AtomicUsize = AtomicUsize::new(0);

/// The player's `DirectDrawCreateEx` hooked: it loads ddraw.dll and finds the function itself.
pub(crate) fn hook_present() {
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        ORIG_GET_PROC.store(o, Ordering::Relaxed);
    }
}

unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> usize {
    type F = unsafe extern "system" fn(P, *const u8) -> usize;
    let orig: F = unsafe { std::mem::transmute(ORIG_GET_PROC.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if f != 0 && name as usize > 0xffff && unsafe { std::ffi::CStr::from_ptr(name.cast()) } == c"DirectDrawCreateEx" {
        ORIG_CREATE_DD.store(f, Ordering::Relaxed);
        return create_dd as *const () as usize;
    }
    f
}

/// `object`'s vtable entry `slot` replaced (once: the vtable is shared), its original kept.
unsafe fn patch_slot(object: P, slot: usize, replacement: usize, orig: &AtomicUsize) {
    let entry = unsafe { (*(object as *const *mut usize)).add(slot) };
    if unsafe { *entry } == replacement {
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(entry.cast(), 4, PAGE_EXECUTE_READWRITE, &mut old);
        orig.store(*entry, Ordering::Relaxed);
        *entry = replacement;
        VirtualProtect(entry.cast(), 4, old, &mut old);
    }
}

unsafe fn method(object: P, slot: usize) -> usize {
    unsafe { *(*(object as *const *const usize)).add(slot) }
}

unsafe extern "system" fn create_dd(guid: P, out: *mut P, iid: P, outer: P) -> i32 {
    type F = unsafe extern "system" fn(P, *mut P, P, P) -> i32;
    let orig: F = unsafe { std::mem::transmute(ORIG_CREATE_DD.load(Ordering::Relaxed)) };
    let r = unsafe { orig(guid, out, iid, outer) };
    if r == 0 && !out.is_null() && unsafe { !(*out).is_null() } {
        DDRAW.store(unsafe { *out } as usize, Ordering::Relaxed);
        unsafe { patch_slot(*out, CREATE_SURFACE, create_surface as *const () as usize, &ORIG_CREATE_SURFACE) };
    }
    r
}

unsafe extern "system" fn create_surface(this: P, desc: *const u8, out: *mut P, outer: P) -> i32 {
    type F = unsafe extern "system" fn(P, *const u8, *mut P, P) -> i32;
    let orig: F = unsafe { std::mem::transmute(ORIG_CREATE_SURFACE.load(Ordering::Relaxed)) };
    let r = unsafe { orig(this, desc, out, outer) };
    // DDSURFACEDESC2.ddsCaps.dwCaps
    if r == 0 && !desc.is_null() && unsafe { (desc.add(0x68) as *const u32).read_unaligned() } & DDSCAPS_PRIMARYSURFACE != 0 {
        let primary = unsafe { *out };
        PRIMARY.store(primary as usize, Ordering::Relaxed);
        unsafe { patch_slot(primary, BLT, blt as *const () as usize, &ORIG_BLT) };
        log!("ultrapin: the DMD composed into the table's picture{}", if rotated() { ", turned upright" } else { "" });
    }
    r
}

type Blt = unsafe extern "system" fn(P, *const RECT, P, *const RECT, u32, P) -> i32;

unsafe extern "system" fn blt(this: P, dst: *const RECT, src: P, src_rect: *const RECT, flags: u32, fx: P) -> i32 {
    let orig: Blt = unsafe { std::mem::transmute(ORIG_BLT.load(Ordering::Relaxed)) };
    if this as usize != PRIMARY.load(Ordering::Relaxed) || src.is_null() || dst.is_null() {
        return unsafe { orig(this, dst, src, src_rect, flags, fx) };
    }
    let Some(size) = (unsafe { surface_size(src) }) else {
        return unsafe { orig(this, dst, src, src_rect, flags, fx) };
    };
    let copied = if src_rect.is_null() { RECT { left: 0, top: 0, right: size.0, bottom: size.1 } } else { unsafe { *src_rect } };
    keep_in_front();
    let new_frame = pace();
    if rotated() {
        // the window's offset on the screen
        let offset = unsafe { ((*dst).left - copied.left, (*dst).top - copied.top) };
        if unsafe { blt_rotated(this, src, size, &copied, offset, new_frame) } {
            return 0;
        }
        return unsafe { orig(this, dst, src, src_rect, flags, fx) };
    }
    let frames = FRAMES.load(Ordering::Relaxed);
    let new = SHOWN.load(Ordering::Relaxed) != frames;
    let dmd = unsafe { draw(src, size, new || intersects(&copied, &DMD_RECT.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(copied))) };
    let r = unsafe { orig(this, dst, src, src_rect, flags, fx) };
    if let Some(dmd) = dmd.filter(|_| new) {
        SHOWN.store(frames, Ordering::Relaxed);
        if !contains(&copied, &dmd) {
            // the window's offset on the screen
            let (dx, dy) = unsafe { ((*dst).left - copied.left, (*dst).top - copied.top) };
            let to = RECT { left: dmd.left + dx, top: dmd.top + dy, right: dmd.right + dx, bottom: dmd.bottom + dy };
            unsafe { orig(this, &to, src, &dmd, flags, std::ptr::null_mut()) };
        }
    }
    r
}

/// The DMD's rectangle in the back buffer, once drawn.
static DMD_RECT: Mutex<Option<RECT>> = Mutex::new(None);

/// The DMD drawn into the back buffer (when `now`), its rectangle.
unsafe fn draw(surface: P, size: (i32, i32), now: bool) -> Option<RECT> {
    let frame = FRAME.lock().unwrap_or_else(|e| e.into_inner());
    let frame = frame.as_ref()?;
    let height = size.1 / 2;
    let width = height * frame.width as i32 / frame.height as i32;
    let rect = RECT { left: 0, top: 0, right: width, bottom: height };
    *DMD_RECT.lock().unwrap_or_else(|e| e.into_inner()) = Some(rect);
    if !now {
        return Some(rect);
    }
    type GetDc = unsafe extern "system" fn(P, *mut HDC) -> i32;
    type ReleaseDc = unsafe extern "system" fn(P, HDC) -> i32;
    let (get_dc, release_dc): (GetDc, ReleaseDc) =
        unsafe { (std::mem::transmute(method(surface, GET_DC)), std::mem::transmute(method(surface, RELEASE_DC))) };
    let mut hdc: HDC = std::ptr::null_mut();
    if unsafe { get_dc(surface, &mut hdc) } != 0 || hdc.is_null() {
        return None;
    }
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: frame.width as i32,
        biHeight: -(frame.height as i32),
        biPlanes: 1,
        biBitCount: 32,
        ..unsafe { std::mem::zeroed() }
    };
    let orig: StretchDIBits = unsafe { std::mem::transmute(gdi_stretch()) };
    unsafe {
        SetStretchBltMode(hdc, HALFTONE);
        orig(hdc, 0, 0, width, height, 0, 0, frame.width as i32, frame.height as i32, frame.pixels.as_ptr().cast(), &info, 0, SRCCOPY);
        release_dc(surface, hdc);
    }
    Some(rect)
}

/// gdi32's own `StretchDIBits` (the hooked import is VPinMAME's).
fn gdi_stretch() -> usize {
    static ADDR: AtomicUsize = AtomicUsize::new(0);
    let mut addr = ADDR.load(Ordering::Relaxed);
    if addr == 0 {
        addr = unsafe { GetProcAddress(GetModuleHandleW("gdi32.dll\0".encode_utf16().collect::<Vec<_>>().as_ptr()), c"StretchDIBits".as_ptr().cast()) }
            .map_or(0, |f| f as usize);
        ADDR.store(addr, Ordering::Relaxed);
    }
    addr
}

/// A surface's width and height (`DDSURFACEDESC2.dwWidth` / `dwHeight`).
unsafe fn surface_size(surface: P) -> Option<(i32, i32)> {
    type F = unsafe extern "system" fn(P, *mut u32) -> i32;
    let get: F = unsafe { std::mem::transmute(method(surface, GET_SURFACE_DESC)) };
    let mut desc = [0u32; 31];
    desc[0] = 124;
    (unsafe { get(surface, desc.as_mut_ptr()) } == 0).then_some((desc[3] as i32, desc[2] as i32))
}

fn intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn contains(a: &RECT, b: &RECT) -> bool {
    a.left <= b.left && a.top <= b.top && a.right >= b.right && a.bottom >= b.bottom
}

/// `WAL_ULTRAPIN_ROTATE`: the table shown upright (default), `0` sideways as in the cabinet.
pub(crate) fn rotated() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("WAL_ULTRAPIN_ROTATE").is_ok_and(|v| v.trim() == "0"))
}

/// A locked surface: its pixels, pitch and pixel format.
struct Locked {
    surface: P,
    /// The locked rectangle (null: the whole surface), its top left corner `bits`.
    rect: Option<RECT>,
    bits: *mut u8,
    pitch: usize,
    format: Format,
}

impl Locked {
    unsafe fn new(surface: P, flags: u32) -> Option<Locked> {
        unsafe { Locked::rect(surface, None, flags) }
    }

    /// The surface's `rect` locked (a lock reads back only it), its pixels still read and
    /// written at the surface's coordinates.
    unsafe fn rect(surface: P, rect: Option<RECT>, flags: u32) -> Option<Locked> {
        type F = unsafe extern "system" fn(P, *const RECT, *mut u32, u32, P) -> i32;
        let lock: F = unsafe { std::mem::transmute(method(surface, LOCK)) };
        let mut desc = [0u32; 31];
        desc[0] = 124;
        let r = rect.as_ref().map_or(std::ptr::null(), |r| r as *const RECT);
        if unsafe { lock(surface, r, desc.as_mut_ptr(), flags | DDLOCK_WAIT, std::ptr::null_mut()) } != 0 {
            return None;
        }
        // lPitch, lpSurface, ddpfPixelFormat.dwRGBBitCount and its masks
        let format = Format { bytes: desc[21] as usize / 8, masks: [desc[22], desc[23], desc[24]] };
        let (ox, oy) = rect.map_or((0, 0), |r| (r.left as usize, r.top as usize));
        let pitch = desc[4] as usize;
        let bits = (desc[9] as usize as *mut u8).wrapping_sub(oy * pitch + ox * format.bytes);
        let locked = Locked { surface, rect, bits, pitch, format };
        (matches!(format.bytes, 2 | 4) && desc[9] != 0).then_some(locked)
    }

    /// The pixel at (x, y) as 0x00RRGGBB.
    unsafe fn get(&self, x: usize, y: usize) -> u32 {
        let at = unsafe { self.bits.add(y * self.pitch + x * self.format.bytes) };
        let p = if self.format.bytes == 4 { unsafe { (at as *const u32).read_unaligned() } } else { u32::from(unsafe { (at as *const u16).read_unaligned() }) };
        self.format.rgb(p)
    }

    unsafe fn set(&self, x: usize, y: usize, rgb: u32) {
        let at = unsafe { self.bits.add(y * self.pitch + x * self.format.bytes) };
        let p = self.format.pixel(rgb);
        if self.format.bytes == 4 {
            unsafe { (at as *mut u32).write_unaligned(p) }
        } else {
            unsafe { (at as *mut u16).write_unaligned(p as u16) }
        }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        type F = unsafe extern "system" fn(P, *const RECT) -> i32;
        let unlock: F = unsafe { std::mem::transmute(method(self.surface, UNLOCK)) };
        let r = self.rect.as_ref().map_or(std::ptr::null(), |r| r as *const RECT);
        unsafe { unlock(self.surface, r) };
    }
}

#[derive(Clone, Copy)]
struct Format {
    bytes: usize,
    masks: [u32; 3],
}

impl Format {
    fn rgb(&self, p: u32) -> u32 {
        if self.masks == [0xff_0000, 0xff00, 0xff] {
            return p & 0xff_ffff;
        }
        rgb16(p, &self.masks)
    }

    fn pixel(&self, rgb: u32) -> u32 {
        if self.masks == [0xff_0000, 0xff00, 0xff] {
            return rgb;
        }
        let channel = |v: u32, mask: u32| {
            let max = mask >> mask.trailing_zeros();
            (v * max / 255) << mask.trailing_zeros()
        };
        channel(rgb >> 16 & 0xff, self.masks[0]) | channel(rgb >> 8 & 0xff, self.masks[1]) | channel(rgb & 0xff, self.masks[2])
    }
}

/// The upright picture: the payload's surface (the back buffer's size), where the table goes.
struct Upright {
    surface: P,
    /// the player's back buffer's size
    size: (i32, i32),
    /// the table's scale and its top left corner
    scale: f32,
    origin: (f32, f32),
    /// the DMD's rectangle and whether it is over the table
    dmd: RECT,
    dmd_over_table: bool,
    /// the upright picture's tiles still to turn (TILE pixels square, a row of `tiles.0`), and
    /// when they last were
    dirty: Vec<bool>,
    tiles: (i32, i32),
    flushed: std::time::Instant,
}

unsafe impl Send for Upright {}

static UPRIGHT: Mutex<Option<Upright>> = Mutex::new(None);

impl Upright {
    unsafe fn new(size: (i32, i32)) -> Option<Upright> {
        // the table fills the screen: the player's window made the screen's size
        let out = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let out = if out.0 > 0 && out.1 > 0 { out } else { size };
        unsafe { fit_window(out) };
        let ddraw = DDRAW.load(Ordering::Relaxed) as P;
        if ddraw.is_null() {
            return None;
        }
        type F = unsafe extern "system" fn(P, *const u32, *mut P, P) -> i32;
        let create: F = unsafe { std::mem::transmute(ORIG_CREATE_SURFACE.load(Ordering::Relaxed)) };
        let mut desc = [0u32; 31];
        desc[0] = 124;
        desc[1] = DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT;
        desc[2] = out.1 as u32;
        desc[3] = out.0 as u32;
        desc[26] = DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY;
        let mut surface: P = std::ptr::null_mut();
        if unsafe { create(ddraw, desc.as_ptr(), &mut surface, std::ptr::null_mut()) } != 0 || surface.is_null() {
            log!("ultrapin: no surface for the upright picture");
            return None;
        }
        if let Some(locked) = unsafe { Locked::new(surface, DDLOCK_WRITEONLY) } {
            for y in 0..out.1 as usize {
                for x in 0..out.0 as usize {
                    unsafe { locked.set(x, y, 0) };
                }
            }
        }
        let (w, h) = (size.0 as f32, size.1 as f32);
        let (ow, oh) = (out.0 as f32, out.1 as f32);
        // upright, the table is the back buffer's height wide and its width high
        let scale = (ow / h).min(oh / w);
        let origin = ((ow - h * scale) / 2.0, (oh - w * scale) / 2.0);
        let margin = origin.0 as i32;
        let pad = 8;
        let (dmd, dmd_over_table) = if margin >= 120 {
            (RECT { left: pad, top: pad, right: margin - pad, bottom: pad + (margin - 2 * pad) / 4 }, false)
        } else {
            let width = (h * scale / 2.0) as i32;
            let left = origin.0 as i32;
            (RECT { left, top: origin.1 as i32, right: left + width, bottom: origin.1 as i32 + width / 4 }, true)
        };
        log!("ultrapin: upright table {}x{} at {},{}, DMD {}x{} at {},{}",
            (h * scale) as i32, (w * scale) as i32, origin.0 as i32, origin.1 as i32,
            dmd.right - dmd.left, dmd.bottom - dmd.top, dmd.left, dmd.top);
        let tiles = ((out.0 + TILE - 1) / TILE, (out.1 + TILE - 1) / TILE);
        let dirty = vec![false; (tiles.0 * tiles.1) as usize];
        Some(Upright { surface, size, scale, origin, dmd, dmd_over_table, dirty, tiles, flushed: std::time::Instant::now() })
    }

    /// The rectangle of the upright picture showing the back buffer's `r`.
    fn turned(&self, r: &RECT) -> RECT {
        let (s, (ox, oy), h) = (self.scale, self.origin, self.size.1 as f32);
        let table = self.table();
        RECT {
            left: ((ox + (h - r.bottom as f32) * s).floor() as i32 - 1).max(table.left),
            top: ((oy + r.left as f32 * s).floor() as i32 - 1).max(table.top),
            right: ((ox + (h - r.top as f32) * s).ceil() as i32 + 1).min(table.right),
            bottom: ((oy + r.right as f32 * s).ceil() as i32 + 1).min(table.bottom),
        }
    }

    /// The table's rectangle in the upright picture.
    fn table(&self) -> RECT {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let (ox, oy) = self.origin;
        RECT { left: ox as i32, top: oy as i32, right: (ox + h * self.scale) as i32, bottom: (oy + w * self.scale) as i32 }
    }

    /// The upright picture's `to` drawn from the back buffer.
    /// The back buffer's rectangle the upright `to` reads (with the bilinear filter's pixels).
    fn source(&self, to: &RECT) -> RECT {
        let (s, (ox, oy)) = (self.scale, self.origin);
        let (w, h) = (self.size.0, self.size.1);
        // upright (u, v) is the back buffer's (v, h - u)
        let left = ((to.top as f32 - oy) / s).floor() as i32 - 1;
        let right = ((to.bottom as f32 - oy) / s).ceil() as i32 + 1;
        let top = h - ((to.right as f32 - ox) / s).ceil() as i32 - 1;
        let bottom = h - ((to.left as f32 - ox) / s).floor() as i32 + 1;
        RECT { left: left.clamp(0, w), top: top.clamp(0, h), right: right.clamp(0, w), bottom: bottom.clamp(0, h) }
    }

    unsafe fn draw_table(&self, back: &Locked, out: &Locked, to: &RECT) {
        let (s, (ox, oy)) = (self.scale, self.origin);
        let (w, h) = (self.size.0, self.size.1);
        for y in to.top..to.bottom {
            for x in to.left..to.right {
                // upright (u, v) is the back buffer's (v, h - u)
                let u = (x as f32 + 0.5 - ox) / s;
                let v = (y as f32 + 0.5 - oy) / s;
                let rgb = bilinear(v - 0.5, h as f32 - u - 0.5, w as usize, h as usize, |bx, by| unsafe { back.get(bx, by) });
                unsafe { out.set(x as usize, y as usize, rgb) };
            }
        }
    }

    /// The DMD drawn upright into its rectangle.
    unsafe fn draw_dmd(&self, out: &Locked) -> bool {
        let frame = FRAME.lock().unwrap_or_else(|e| e.into_inner());
        let Some(frame) = frame.as_ref() else { return false };
        let r = self.dmd;
        // upright, the DMD is the frame's height wide and its width high, fitted in r
        let (fw, fh) = (frame.width as f32, frame.height as f32);
        let d = ((r.right - r.left) as f32 / fh).min((r.bottom - r.top) as f32 / fw);
        let (width, height) = ((fh * d) as i32, (fw * d) as i32);
        for y in r.top..r.top + height {
            for x in r.left..r.left + width {
                let a = (x - r.left) as f32 / d;
                let b = (y - r.top) as f32 / d;
                let rgb = bilinear(b, fh - a - 1.0, frame.width, frame.height, |fx, fy| frame.pixels[fy * frame.width + fx]);
                unsafe { out.set(x as usize, y as usize, rgb) };
            }
        }
        true
    }
}

/// The color at (x, y) between the four nearest pixels.
fn bilinear(x: f32, y: f32, width: usize, height: usize, pixel: impl Fn(usize, usize) -> u32) -> u32 {
    let x = x.clamp(0.0, (width - 1) as f32);
    let y = y.clamp(0.0, (height - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (a, b, c, d) = (pixel(x0, y0), pixel(x1, y0), pixel(x0, y1), pixel(x1, y1));
    let mut rgb = 0;
    for shift in [16, 8, 0] {
        let ch = |p: u32| (p >> shift & 0xff) as f32;
        let top = ch(a) + (ch(b) - ch(a)) * fx;
        let bottom = ch(c) + (ch(d) - ch(c)) * fx;
        rgb |= ((top + (bottom - top) * fy + 0.5) as u32).min(255) << shift;
    }
    rgb
}

/// The player's copy of `copied` to the screen, made upright: false when it could not be.
///
/// The player copies many small rectangles a frame, and each lock of its back buffer reads it
/// back: the upright picture's tiles they cover are marked, and turned together once a frame
/// (`new_frame`, its first copy), or after [`FLUSH`].
unsafe fn blt_rotated(primary: P, back: P, size: (i32, i32), copied: &RECT, offset: (i32, i32), new_frame: bool) -> bool {
    let mut upright = UPRIGHT.lock().unwrap_or_else(|e| e.into_inner());
    if upright.as_ref().is_none_or(|u| u.size != size) {
        *upright = unsafe { Upright::new(size) };
    }
    let Some(up) = upright.as_mut() else { return false };
    let to = up.turned(copied);
    for ty in to.top / TILE..(to.bottom + TILE - 1) / TILE {
        for tx in to.left / TILE..(to.right + TILE - 1) / TILE {
            up.dirty[(ty * up.tiles.0 + tx) as usize] = true;
        }
    }
    if !new_frame && up.flushed.elapsed() < FLUSH {
        return true;
    }
    up.flushed = std::time::Instant::now();
    let table = up.table();
    let mut pending = Vec::new();
    for (i, dirty) in up.dirty.iter_mut().enumerate() {
        if std::mem::take(dirty) {
            let (tx, ty) = (i as i32 % up.tiles.0, i as i32 / up.tiles.0);
            let tile = RECT {
                left: (tx * TILE).max(table.left),
                top: (ty * TILE).max(table.top),
                right: ((tx + 1) * TILE).min(table.right),
                bottom: ((ty + 1) * TILE).min(table.bottom),
            };
            if tile.right > tile.left && tile.bottom > tile.top {
                pending.push(tile);
            }
        }
    }
    let frames = FRAMES.load(Ordering::Relaxed);
    let new_dmd = SHOWN.load(Ordering::Relaxed) != frames;
    let mut shown: Option<RECT> = None;
    {
        let tiles = pending.iter().copied().reduce(|a, b| union(&a, &b));
        let source = tiles.map(|t| up.source(&t));
        let Some(src) = (unsafe { Locked::rect(back, source, DDLOCK_READONLY) }) else { return false };
        let Some(out) = (unsafe { Locked::new(up.surface, DDLOCK_WRITEONLY) }) else { return false };
        for to in &pending {
            unsafe { up.draw_table(&src, &out, to) };
            shown = Some(shown.map_or(*to, |s| union(&s, to)));
        }
        let covered = shown.is_some_and(|s| up.dmd_over_table && intersects(&s, &up.dmd));
        if (new_dmd || covered) && unsafe { up.draw_dmd(&out) } {
            SHOWN.store(frames, Ordering::Relaxed);
            shown = Some(shown.map_or(up.dmd, |s| union(&s, &up.dmd)));
        }
    }
    if let Some(r) = shown {
        let orig: Blt = unsafe { std::mem::transmute(ORIG_BLT.load(Ordering::Relaxed)) };
        let screen = RECT { left: r.left + offset.0, top: r.top + offset.1, right: r.right + offset.0, bottom: r.bottom + offset.1 };
        unsafe { orig(primary, &screen, up.surface, &r, DDBLT_WAIT, std::ptr::null_mut()) };
    }
    true
}

/// The player's window (`VPPlayer`) made the screen's `size`, at its top left corner.
unsafe fn fit_window(size: (i32, i32)) {
    let window = unsafe { table_window() };
    if window.is_null() {
        log!("ultrapin: the table's window not found");
        return;
    }
    unsafe { SetWindowPos(window, std::ptr::null_mut(), 0, 0, size.0, size.1, SWP_NOZORDER | SWP_NOACTIVATE) };
}

/// The player's window (class `VPPlayer`), null before it is made.
unsafe fn table_window() -> HWND {
    unsafe extern "system" fn find(hwnd: HWND, out: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let mut class = [0u16; 16];
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), 16) } as usize;
        if pid == unsafe { GetCurrentProcessId() } && String::from_utf16_lossy(&class[..n]) == "VPPlayer" {
            unsafe { *(out as *mut HWND) = hwnd };
            return 0;
        }
        1
    }
    let mut window: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(find), &mut window as *mut HWND as LPARAM) };
    window
}

/// When the player's windows were last checked.
static FRONT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The player's windows: the table's kept on top (it can open behind the frontend's loading
/// picture, the table then playing in a black screen; the desktop refuses it the focus), the
/// editor's (`VPinball`, the player started minimized) and VPinMAME's DMD (`MAME`, drawn into the
/// table's) hidden; checked twice a second.
fn keep_in_front() {
    let mut front = FRONT.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if front.is_some_and(|checked| now - checked < std::time::Duration::from_millis(500)) {
        return;
    }
    *front = Some(now);
    unsafe extern "system" fn check(hwnd: HWND, _: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != unsafe { GetCurrentProcessId() } || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut class = [0u16; 16];
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), 16) } as usize;
        match String::from_utf16_lossy(&class[..n]).as_str() {
            "VPPlayer" if unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST == 0 => {
                unsafe { SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE) };
                log!("ultrapin: the table's window kept on top");
            }
            class @ ("VPinball" | "MAME") => {
                unsafe { ShowWindowAsync(hwnd, SW_HIDE) };
                log!("ultrapin: the {class} window hidden");
            }
            _ => {}
        }
        1
    }
    unsafe { EnumWindows(Some(check), 0) };
}

/// The upright picture's tiles (pixels square).
const TILE: i32 = 32;

/// The longest time between two turns of the queued rectangles (the player's frames not
/// counted).
const FLUSH: std::time::Duration = std::time::Duration::from_millis(50);

/// The player's frame counter (`Player`, at `0x526a64`, its frames at +0x9c0, counted after a
/// frame's copies to the screen).
fn frame_count() -> Option<u32> {
    let player = unsafe { std::ptr::read_volatile(0x526a64 as *const usize) };
    (player != 0).then(|| unsafe { std::ptr::read_volatile((player + 0x9c0) as *const u32) })
}

/// The last frame counted and when the next one is due.
static PACE: Mutex<Option<(u32, std::time::Instant)>> = Mutex::new(None);

/// The player's frames paced to the screen's refresh (it draws thousands a second, unthrottled,
/// its physics in steps of their own: the GPU's queue drained, each frame shown): true at a
/// frame's first copy.
fn pace() -> bool {
    let Some(count) = frame_count() else { return false };
    let mut pace = PACE.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let (last, due) = pace.get_or_insert((count, now));
    if *last == count {
        return false;
    }
    *last = count;
    if now < *due {
        std::thread::sleep(*due - now);
    }
    // the next frame due a refresh later, not catching up after a late one
    *due = (*due + refresh()).max(now);
    true
}

/// The screen's refresh period (60 Hz when unknown).
fn refresh() -> std::time::Duration {
    static PERIOD: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
    *PERIOD.get_or_init(|| {
        let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
        mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        let hz = if unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut mode) } != 0 && mode.dmDisplayFrequency > 1 {
            mode.dmDisplayFrequency
        } else {
            60
        };
        log!("ultrapin: the table's frames paced to {hz} Hz");
        std::time::Duration::from_secs_f64(1.0 / hz as f64)
    })
}

fn union(a: &RECT, b: &RECT) -> RECT {
    RECT { left: a.left.min(b.left), top: a.top.min(b.top), right: a.right.max(b.right), bottom: a.bottom.max(b.bottom) }
}
