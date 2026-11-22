//! The native Sinden frame of DirectDraw games presenting with a `Blt` of their back buffer to
//! the primary surface (Dream Hunting). With the frame the display mode is not changed and the
//! game's window is screen-sized (window.rs): the primary is the screen's size and the game's
//! `Blt` drew its mode's picture 1:1 in its top left corner, no frame. That `Blt` is drawn
//! instead into a primary-sized surface of ours holding the frame (drawn at its creation), the
//! back buffer scaled into the picture inside the frame (its aspect kept: [`picture`]), then
//! copied whole to the primary. `IDirectDrawSurface`'s `Blt` is hooked in its vtables (one per
//! interface version in Wine's ddraw.dll), found on a surface of a DirectDraw object of ours;
//! [`init`] goes before [`crate::crosshair::init_ddraw`] (whose `Blt` then calls this one).
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

type P = *mut c_void;
type Blt = unsafe extern "system" fn(P, *const [i32; 4], P, *const [i32; 4], u32, P) -> i32;

/// `IDirectDrawSurface` vtable slots.
const BLT: usize = 5;
const GET_CAPS: usize = 14;
const GET_SURFACE_DESC: usize = 22;
const GET_DD_INTERFACE: usize = 36;
const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDBLT_COLORFILL: u32 = 0x400;
const DDBLT_WAIT: u32 = 0x100_0000;

/// `Blt` of each surface interface (1, 2, 3, 4, 7).
static ORIGINAL: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];
/// IDirectDrawSurface, 2, 3, 4, 7.
const IIDS: [[u8; 16]; 5] = [
    guid(0x6c14_db81, 0xa733, 0x11ce, [0xa5, 0x21, 0x00, 0x20, 0xaf, 0x0b, 0xe5, 0x60]),
    guid(0x5780_5885, 0x6eec, 0x11cf, [0x94, 0x41, 0xa8, 0x23, 0x03, 0xc1, 0x0e, 0x27]),
    guid(0xda04_4e00, 0x69b2, 0x11d0, [0xa1, 0xd5, 0x00, 0xaa, 0x00, 0xb8, 0xdf, 0xbb]),
    guid(0x0b2b_8630, 0xad35, 0x11d0, [0x8e, 0xa6, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b]),
    guid(0x0667_5a80, 0x3b9b, 0x11d2, [0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b]),
];
const IID_IDIRECTDRAW7: [u8; 16] = guid(0x15e6_5ec0, 0x3b9c, 0x11d2, [0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b]);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let (a, b, c) = (a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes());
    [a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]
}

macro_rules! com {
    ($obj:expr, $slot:expr, fn($($t:ty),*) -> $ret:ty $(, $arg:expr)*) => {{
        let obj = $obj;
        let f: unsafe extern "system" fn(P $(, $t)*) -> $ret = unsafe { std::mem::transmute(*(*(obj as *const *const usize)).add($slot)) };
        unsafe { f(obj $(, $arg)*) }
    }};
}

/// The surface of ours the picture is composed in.
struct Composed {
    /// Its interface of the game's version (`Blt` to the primary takes the same).
    surface: usize,
    version: usize,
    /// The primary's size.
    size: (u32, u32),
}

unsafe impl Send for Composed {}

static COMPOSED: Mutex<Option<Composed>> = Mutex::new(None);

/// The picture's rectangle (left, top, right, bottom) of a `aw`:`ah` source on a `w` x `h`
/// screen with the frame: the largest centered one in the frame's opening. `None` without the
/// frame.
pub fn picture(w: u32, h: u32, aw: u32, ah: u32) -> Option<[i32; 4]> {
    let ([l, t, r, b], ..) = crate::border::geometry(w, h)?;
    let (iw, ih) = ((r - l) as i64, (b - t) as i64);
    let (aw, ah) = (aw.max(1) as i64, ah.max(1) as i64);
    let (pw, ph) = if iw * ah > ih * aw { (ih * aw / ah, ih) } else { (iw, iw * ah / aw) };
    let (x, y) = (l + ((iw - pw) / 2) as i32, t + ((ih - ph) / 2) as i32);
    Some([x, y, x + pw as i32, y + ph as i32])
}

/// An 8-bit channel at `mask`'s bits.
fn channel(c: u32, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let max = mask >> mask.trailing_zeros();
    ((c * max + 127) / 255) << mask.trailing_zeros() & mask
}

/// A color fill of `rect` of `surface` (interface version `v`).
unsafe fn fill(surface: P, v: usize, rect: Option<[i32; 4]>, color: u32) -> i32 {
    // DDBLTFX (100 bytes): dwFillColor at 80
    let mut fx = [0u32; 25];
    fx[0] = 100;
    fx[20] = color;
    let original: Blt = unsafe { std::mem::transmute(ORIGINAL[v].load(Ordering::Relaxed)) };
    let rect = rect.filter(|r| r[2] > r[0] && r[3] > r[1]);
    unsafe { original(surface, rect.as_ref().map_or(std::ptr::null(), |r| r), std::ptr::null_mut(), std::ptr::null(), DDBLT_COLORFILL | DDBLT_WAIT, fx.as_mut_ptr().cast()) }
}

/// A surface of `primary`'s DirectDraw object, its size and pixel format (`desc`: its
/// DDSURFACEDESC(2)), with the frame drawn: its interface of version `v`.
unsafe fn compose_surface(primary: P, v: usize, desc: &[u32; 31]) -> Option<usize> {
    let (w, h) = (desc[3], desc[2]);
    let mut surface7: P = std::ptr::null_mut();
    if com!(primary, 0, fn(*const [u8; 16], *mut P) -> i32, &IIDS[4], &mut surface7) < 0 || surface7.is_null() {
        return None;
    }
    let mut unknown: P = std::ptr::null_mut();
    let hr = com!(surface7, GET_DD_INTERFACE, fn(*mut P) -> i32, &mut unknown);
    com!(surface7, 2, fn() -> u32);
    if hr < 0 || unknown.is_null() {
        return None;
    }
    let mut dd7: P = std::ptr::null_mut();
    let hr = com!(unknown, 0, fn(*const [u8; 16], *mut P) -> i32, &IID_IDIRECTDRAW7, &mut dd7);
    com!(unknown, 2, fn() -> u32);
    if hr < 0 || dd7.is_null() {
        return None;
    }
    // DDSURFACEDESC2: DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT, the primary's
    // pixel format (72, 32 bytes), DDSCAPS_OFFSCREENPLAIN | DDSCAPS_VIDEOMEMORY
    let mut new = [0u32; 31];
    new[0] = 124;
    new[1] = 0x1 | 0x2 | 0x4 | 0x1000;
    new[2] = h;
    new[3] = w;
    new[18..26].copy_from_slice(&desc[18..26]);
    new[26] = 0x40 | 0x4000;
    let mut created: P = std::ptr::null_mut();
    let hr = com!(dd7, 6, fn(*mut [u32; 31], *mut P, P) -> i32, &mut new, &mut created, std::ptr::null_mut());
    com!(dd7, 2, fn() -> u32);
    if hr < 0 || created.is_null() {
        log!("sinden: no {w}x{h} DirectDraw surface ({hr:#x}), no frame");
        return None;
    }
    let mut surface: P = std::ptr::null_mut();
    let hr = com!(created, 0, fn(*const [u8; 16], *mut P) -> i32, &IIDS[v], &mut surface);
    com!(created, 2, fn() -> u32);
    if hr < 0 || surface.is_null() {
        return None;
    }
    // black, then the colored ring (its black outline left)
    let Some((_, bx, by, rgb)) = crate::border::geometry(w, h) else {
        com!(surface, 2, fn() -> u32);
        return None;
    };
    let (ox, oy) = crate::border::outline(w, h);
    let color = channel(rgb >> 16 & 0xff, desc[22]) | channel(rgb >> 8 & 0xff, desc[23]) | channel(rgb & 0xff, desc[24]);
    unsafe { fill(surface, v, None, 0) };
    for strip in crate::border::strips(w as i32, h as i32, bx, by, ox, oy) {
        unsafe { fill(surface, v, Some(strip), color) };
    }
    Some(surface as usize)
}

/// The back buffer `src` (`src_rect`) about to be copied to `dst_rect` of the primary `dst`:
/// composed with the frame and copied to the primary instead, the result (`None`: not done).
unsafe fn present(dst: P, src: P, src_rect: *const [i32; 4], v: usize) -> Option<i32> {
    let mut caps = [0u32; 4];
    if com!(dst, GET_CAPS, fn(*mut [u32; 4]) -> i32, &mut caps) < 0 || caps[0] & DDSCAPS_PRIMARYSURFACE == 0 {
        return None;
    }
    // DDSURFACEDESC (108 bytes), DDSURFACEDESC2 (124) for the interfaces 4 and 7: height 8,
    // width 12, pixel format 72 (red / green / blue masks 88 / 92 / 96)
    let mut desc = [0u32; 31];
    desc[0] = if v >= 3 { 124 } else { 108 };
    if com!(dst, GET_SURFACE_DESC, fn(*mut [u32; 31]) -> i32, &mut desc) < 0 {
        return None;
    }
    let (w, h) = (desc[3], desc[2]);
    let source = if src_rect.is_null() {
        let mut s = [0u32; 31];
        s[0] = desc[0];
        if com!(src, GET_SURFACE_DESC, fn(*mut [u32; 31]) -> i32, &mut s) < 0 {
            return None;
        }
        [0, 0, s[3] as i32, s[2] as i32]
    } else {
        unsafe { *src_rect }
    };
    let (sw, sh) = ((source[2] - source[0]).max(1) as u32, (source[3] - source[1]).max(1) as u32);
    let rect = picture(w, h, sw, sh)?;
    let mut guard = COMPOSED.lock().unwrap();
    if guard.as_ref().is_some_and(|c| c.size != (w, h) || c.version != v) {
        let old = guard.take().unwrap();
        com!(old.surface as P, 2, fn() -> u32);
    }
    if guard.is_none() {
        let surface = unsafe { compose_surface(dst, v, &desc) }?;
        let [l, t, r, b] = rect;
        log!("sinden: DirectDraw {sw}x{sh} picture scaled to {}x{} at {l},{t} in the frame of the {w}x{h} primary", r - l, b - t);
        *guard = Some(Composed { surface, version: v, size: (w, h) });
    }
    let composed = guard.as_ref()?.surface as P;
    let original: Blt = unsafe { std::mem::transmute(ORIGINAL[v].load(Ordering::Relaxed)) };
    let hr = unsafe { original(composed, &rect, src, &source, DDBLT_WAIT, std::ptr::null_mut()) };
    if hr < 0 {
        // lost (a mode or device change): made again
        let old = guard.take().unwrap();
        com!(old.surface as P, 2, fn() -> u32);
        return None;
    }
    Some(unsafe { original(dst, std::ptr::null(), composed, std::ptr::null(), DDBLT_WAIT, std::ptr::null_mut()) })
}

unsafe extern "system" fn blt<const V: usize>(dst: P, dst_rect: *const [i32; 4], src: P, src_rect: *const [i32; 4], flags: u32, fx: P) -> i32 {
    if !src.is_null() && src != dst && flags & DDBLT_COLORFILL == 0 {
        if let Some(hr) = unsafe { present(dst, src, src_rect, V) } {
            return hr;
        }
    }
    let original: Blt = unsafe { std::mem::transmute(ORIGINAL[V].load(Ordering::Relaxed)) };
    unsafe { original(dst, dst_rect, src, src_rect, flags, fx) }
}

/// `Blt` replaced in `surface`'s vtable by `f`.
unsafe fn patch(surface: P, f: usize, original: &AtomicUsize) {
    unsafe {
        let slot = (*(surface as *const *mut usize)).add(BLT);
        let mut old = 0;
        VirtualProtect(slot as *const _, 4, PAGE_EXECUTE_READWRITE, &mut old);
        if *slot != f {
            original.store(*slot, Ordering::Relaxed);
            *slot = f;
        }
        VirtualProtect(slot as *const _, 4, old, &mut old);
    }
}

/// The frame of a DirectDraw game presenting with a `Blt` to its primary surface, when the
/// frame is on. Whether it is drawn.
#[allow(unused_unsafe)]
pub fn init() -> bool {
    if crate::border::config().is_none() {
        return false;
    }
    unsafe {
        let mut module = GetModuleHandleA(c"ddraw.dll".as_ptr().cast());
        if module.is_null() {
            module = LoadLibraryA(c"ddraw.dll".as_ptr().cast());
        }
        let Some(create) = GetProcAddress(module, c"DirectDrawCreate".as_ptr().cast()) else {
            log!("sinden: no DirectDrawCreate, no frame");
            return false;
        };
        let create: unsafe extern "system" fn(P, *mut P, P) -> i32 = std::mem::transmute(create);
        let mut dd: P = std::ptr::null_mut();
        if create(std::ptr::null_mut(), &mut dd, std::ptr::null_mut()) < 0 || dd.is_null() {
            log!("sinden: DirectDrawCreate failed, no frame");
            return false;
        }
        // SetCooperativeLevel(NULL, DDSCL_NORMAL), CreateSurface (an 8x8 offscreen surface)
        com!(dd, 20, fn(P, u32) -> i32, std::ptr::null_mut(), 0x8);
        let mut desc = [0u32; 27];
        desc[0] = 108;
        desc[1] = 0x1 | 0x2 | 0x4; // DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH
        desc[2] = 8;
        desc[3] = 8;
        desc[26] = 0x40 | 0x800; // DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY
        let mut surface: P = std::ptr::null_mut();
        let hr = com!(dd, 6, fn(*mut [u32; 27], *mut P, P) -> i32, &mut desc, &mut surface, std::ptr::null_mut());
        if hr < 0 || surface.is_null() {
            log!("sinden: no DirectDraw surface ({hr:#x}), no frame");
            com!(dd, 2, fn() -> u32);
            return false;
        }
        let blts: [usize; 5] = [blt::<0> as *const () as usize, blt::<1> as *const () as usize, blt::<2> as *const () as usize, blt::<3> as *const () as usize, blt::<4> as *const () as usize];
        patch(surface, blts[0], &ORIGINAL[0]);
        for (i, iid) in IIDS.iter().enumerate().skip(1) {
            let mut other: P = std::ptr::null_mut();
            if com!(surface, 0, fn(*const [u8; 16], *mut P) -> i32, iid, &mut other) >= 0 && !other.is_null() {
                patch(other, blts[i], &ORIGINAL[i]);
                com!(other, 2, fn() -> u32);
            }
        }
        com!(surface, 2, fn() -> u32);
        com!(dd, 2, fn() -> u32);
    }
    log!("sinden: frame drawn around the DirectDraw primary surface's picture");
    true
}
