//! Lost Land Adventure's dome picture. The game renders its view into a 3520x1980 back buffer:
//! two 90 degree perspective views side by side (each 1760x1980, square pixels, yawed 45
//! degrees left and right: 180 x 97 degrees in all; a straight object kinks at the seam). It
//! then draws the screen through a lookup texture of the same size (`Startup.upk`, per texel
//! two 16-bit big-endian texture coordinates: u left to right, v bottom to top) made for the
//! cabinet's dome projector. A known dump patch (`CreateDomeMap`) puts in a rectilinear view
//! of about 127 x 97 degrees, flat but stretched on the sides.
//!
//! The payload replaces that texture's bytes as the game reads them (`CreateFileW` / `ReadFile`
//! of the executable) with one rectilinear view, `WAL_NAMCOES3_FOV` degrees wide (default 110,
//! the height from a 16:9 screen) ([`texel`]). It is found from its UE3 bulk data header (16
//! bytes: flags, element count, size, file offset), so only an uncompressed `Startup.upk` laid
//! out as expected is changed, whether the dome or the patched texture is there.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

const WIDTH: usize = 3520;
const HEIGHT: usize = 1980;
const SIZE: u64 = (WIDTH * HEIGHT * 4) as u64;
/// The texture's data in the uncompressed `Startup.upk`, and its bulk data header just before.
const OFFSET: u64 = 0x7f3950d;
const HEADER: [u8; 16] = [0, 0, 0, 0, 0x00, 0x64, 0xa9, 0x01, 0x00, 0x64, 0xa9, 0x01, 0x0d, 0x95, 0xf3, 0x07];

static CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static READ_FILE: AtomicUsize = AtomicUsize::new(0);
static SET_FILE_POINTER: AtomicUsize = AtomicUsize::new(0);
/// Handles of `Startup.upk` the game opened.
static HANDLES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void;
type ReadFile = unsafe extern "system" fn(*mut c_void, *mut u8, u32, *mut u32, *mut c_void) -> i32;
type SetFilePointer = unsafe extern "system" fn(*mut c_void, i32, *mut i32, u32) -> u32;

/// Tangents of the half angles of the flat view.
struct View {
    tan_h: f32,
    tan_v: f32,
}

fn view() -> &'static View {
    static VIEW: std::sync::OnceLock<View> = std::sync::OnceLock::new();
    VIEW.get_or_init(|| {
        let fov = std::env::var("WAL_NAMCOES3_FOV").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(110.0).clamp(30.0, 170.0);
        let tan_h = (fov.to_radians() / 2.0).tan();
        View { tan_h, tan_v: tan_h * 9.0 / 16.0 }
    })
}

/// The back buffer views' vertical half angle tangent (1760x1980 square pixels, 90 degrees wide).
const TAN_BB_V: f32 = 1980.0 / 1760.0;
const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Back buffer position (u, w: -1..1, w up) of a direction (x right, y up, z ahead).
fn back_buffer(d: [f32; 3]) -> (f32, f32) {
    // the left view looks 45 degrees left, the right view 45 degrees right
    let (xc, zc, center) = if d[0] < 0.0 { ((d[0] + d[2]) * S, (d[2] - d[0]) * S, -0.5) } else { ((d[0] - d[2]) * S, (d[0] + d[2]) * S, 0.5) };
    (center + 0.5 * xc / zc, d[1] / zc / TAN_BB_V)
}

/// The direction of a back buffer position.
fn direction(u: f32, w: f32) -> [f32; 3] {
    let left = u < 0.0;
    let xc = (u - if left { -0.5 } else { 0.5 }) * 2.0;
    let (y, zc) = (w * TAN_BB_V, 1.0);
    if left { [(xc - zc) * S, y, (xc + zc) * S] } else { [(xc + zc) * S, y, (zc - xc) * S] }
}

/// The texture coordinates (0..1, v from the bottom) the screen point `(x, y)` (0..1 from the
/// top left corner) shows.
fn texel(x: f32, y: f32) -> (f32, f32) {
    let v = view();
    let (u, w) = back_buffer([(2.0 * x - 1.0) * v.tan_h, (1.0 - 2.0 * y) * v.tan_v, 1.0]);
    ((u + 1.0) / 2.0, (w + 1.0) / 2.0)
}

/// Where a point the patched dome texture shows at `(x, y)` (0..1 from the top left corner) is
/// on our flat view.
pub(crate) fn from_patched_view(x: f32, y: f32) -> (f32, f32) {
    // the patch's mapping (CreateDomeMap): screen -> back buffer
    let x = 2.0 * x - 1.0;
    let f = 0.95 * x.abs() / (x.abs() + 0.5);
    let (u, w) = (f.copysign(x), (1.0 - 2.0 * y) * (1.0 - f));
    let d = direction(u, w);
    let v = view();
    (0.5 + 0.5 * d[0] / d[2] / v.tan_h, 0.5 - 0.5 * d[1] / d[2] / v.tan_v)
}

/// The replacement texture bytes in `range` (offsets in the texture).
fn fill(out: &mut [u8], start: u64) {
    for (i, b) in out.iter_mut().enumerate() {
        let at = start as usize + i;
        let (texel_index, byte) = (at / 4, at % 4);
        let (x, y) = (texel_index % WIDTH, texel_index / WIDTH);
        let (u, v) = texel(x as f32 / (WIDTH - 1) as f32, y as f32 / (HEIGHT - 1) as f32);
        let q = |f: f32| (f.clamp(0.0, 1.0) * 65535.0).round() as u16;
        let word = if byte < 2 { q(u) } else { q(v) };
        *b = word.to_be_bytes()[byte % 2];
    }
}

unsafe extern "system" fn create_file(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, template: *mut c_void) -> *mut c_void {
    let orig: CreateFileW = unsafe { std::mem::transmute(CREATE_FILE.load(Ordering::Relaxed)) };
    let h = unsafe { orig(name, access, share, sa, disp, flags, template) };
    if !name.is_null() && h as isize != -1 {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) });
        if path.to_ascii_lowercase().ends_with("\\startup.upk") {
            HANDLES.lock().unwrap().push(h as usize);
        }
    }
    h
}

unsafe extern "system" fn read_file(h: *mut c_void, buf: *mut u8, n: u32, read: *mut u32, ov: *mut c_void) -> i32 {
    let orig: ReadFile = unsafe { std::mem::transmute(READ_FILE.load(Ordering::Relaxed)) };
    if ov.is_null() && HANDLES.lock().unwrap().contains(&(h as usize)) {
        let seek: SetFilePointer = unsafe { std::mem::transmute(SET_FILE_POINTER.load(Ordering::Relaxed)) };
        let mut high = 0i32;
        let low = unsafe { seek(h, 0, &mut high, 1) };
        let pos = (high as u64) << 32 | low as u64;
        let ok = unsafe { orig(h, buf, n, read, ov) };
        let got = if read.is_null() { 0 } else { unsafe { *read } } as u64;
        if ok != 0 && pos < OFFSET + SIZE && pos + got > OFFSET {
            let data = unsafe { std::slice::from_raw_parts_mut(buf, got as usize) };
            // the bulk data header, when this read has it
            if pos <= OFFSET - 16 && pos + got >= OFFSET {
                let at = (OFFSET - 16 - pos) as usize;
                if data[at..at + 16] != HEADER {
                    log!("dome: Startup.upk has another layout: dome map kept");
                    HANDLES.lock().unwrap().retain(|x| *x != h as usize);
                    return ok;
                }
                log!("dome: flat view, {:.0} degrees wide", (view().tan_h.atan() * 2.0).to_degrees());
            }
            let from = OFFSET.max(pos);
            let to = (OFFSET + SIZE).min(pos + got);
            fill(&mut data[(from - pos) as usize..(to - pos) as usize], from - OFFSET);
        }
        return ok;
    }
    unsafe { orig(h, buf, n, read, ov) }
}

pub(crate) fn init(base: usize) {
    unsafe {
        for (name, f, slot) in [
            ("CreateFileW", create_file as *const () as usize, &CREATE_FILE),
            ("ReadFile", read_file as *const () as usize, &READ_FILE),
        ] {
            match iat::hook_module(base, "kernel32.dll", name, f) {
                Some(o) => slot.store(o, Ordering::Relaxed),
                None => log!("dome: {name} not hooked"),
            }
        }
    }
    let kernel32 = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(windows_sys::w!("kernel32.dll")) };
    if let Some(p) = unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(kernel32, c"SetFilePointer".as_ptr().cast()) } {
        SET_FILE_POINTER.store(p as usize, Ordering::Relaxed);
    }
}
