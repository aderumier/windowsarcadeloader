//! Yu-Gi-Oh! 5D's Duel Terminal 6: `contents/exe/game.exe` (with `libpenteltp.dll`, the touch
//! panel, `libcamera.dll`, the card camera, `libio.dll` over this payload's HBHI board, card
//! dispenser and card reader). The cabinet keeps the game in `D:\jd6`: D: is the dump root
//! (`..\..` from the executable, `WAL_KONAMI_DDRIVE`), `jd6` skipped.
//!
//! The touch panel (Pentel, COM1, libpenteltp's serial protocol): the start command `81 02`
//! is answered with the two status responses the boot check wants (`C1 03 00`, `C2 0E 00 00`
//! and 10 bytes), then the panel streams 8-byte coordinate packets until the stop command
//! `82 02`: `82`, the touch flag (bit 0 of byte 1), X and Y (bytes 4-5, 6-7: 7 bits each, low
//! first, 14 bits). The touch is player 1's right stick position over the screen (the
//! profile's mouse or gun drives `rx`/`ry`) and its button 4 (the mouse button), as the other
//! touch screens (`touchpanel.rs`); the 1280x1024 picture is fitted in the screen.
//!
//! The dump's libpenteltp has two jumps of its observer's update nopped: every coordinate
//! packet after an untouched one made a touch edge, whatever its flag, and the release edge
//! never came (a single touch did both calibration points, the check got nothing): they are
//! put back. Its poll parses one packet per call (also when nothing was read): the panel
//! sends one per read.
//!
//! The touch calibration (test menu, touch panel check, test + service held): a new cabinet
//! stops at its notice until the panel is calibrated. The panel is exact here: with none saved,
//! the game is given the calibration of its two targets touched dead on (and calibrated); one
//! made in the test menu is used as is.
//!
//! The card camera (`libcamera.dll`, DirectShow, none here): the game's imports answered, one
//! camera of 640x480 RGB24 images, every call done (status 0), blank pictures.

use wal_payload_common::{drive, iat, log, serial};
use wal_protocol::{Axis, button};
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryA};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

/// The game: an executable with the Pentel touch panel's library.
pub fn detected() -> bool {
    let name: Vec<u16> = "libpenteltp.dll\0".encode_utf16().collect();
    !unsafe { GetModuleHandleW(name.as_ptr()) }.is_null()
}

pub fn init() {
    drive::init("WAL_KONAMI_DDRIVE", "..\\..");
    drive::skip_folder("jd6");
    // its libraries open their data themselves
    for module in ["libavs-win32.dll", "libavs-win32-ea3.dll", "libgraph.dll", "libaep.dll", "libmv.dll", "sound.dll", "libamsd.dll", "libutility.dll", "libcamera.dll", "libio.dll", "qrlib.dll", "ygd-code.dll"] {
        drive::redirect_module(module);
    }
    // libmv's DirectShow graph: quartz checks the movie, its WM reader (winedmo) opens it
    for module in [c"quartz.dll", c"winedmo.dll"] {
        unsafe { LoadLibraryA(module.as_ptr().cast()) };
        drive::redirect_module(module.to_str().unwrap());
    }
    crate::hbhi::set_dispenser();
    let name: Vec<u16> = "libpenteltp.dll\0".encode_utf16().collect();
    let panel = unsafe { GetModuleHandleW(name.as_ptr()) } as usize;
    serial::install_streaming_in("COM1", panel_command, panel_stream, &[panel]);
    // the touch pointer shown (WAL_KONAMI_CROSSHAIRS)
    crate::crosshair::init();
    restore_edges(panel);
    default_calibration();
    for (name, f) in [
        ("?LibCameraInit@@YAXXZ", camera_init as *const () as usize),
        ("?LibCameraOpen@@YA?AW4LIBCAMERA_STATUS@@N@Z", camera_open as *const () as usize),
        ("?LibCameraRun@@YA?AW4LIBCAMERA_STATUS@@XZ", camera_ok as *const () as usize),
        ("?LibCameraStop@@YA?AW4LIBCAMERA_STATUS@@XZ", camera_ok as *const () as usize),
        ("?LibCameraClose@@YAXXZ", camera_init as *const () as usize),
        ("?LibCameraGetCameraNr@@YAHXZ", camera_count as *const () as usize),
        ("?LibCameraGetBufferSize@@YAHH@Z", camera_buffer_size as *const () as usize),
        ("?LibCameraGetImage@@YA?AW4LIBCAMERA_STATUS@@HPAX_N@Z", camera_image as *const () as usize),
    ] {
        if unsafe { iat::hook("libcamera.dll", name, f) }.is_none() {
            log!("ygo: {name} not imported");
        }
    }
    log!("ygo: D:\\jd6 is the dump, card dispenser present");
}

/// The panel's answer to a command: the start command's two status responses.
fn panel_command(packet: &[u8]) -> Vec<u8> {
    if packet.starts_with(&[0x82, 0x02]) {
        STREAMING.store(false, Ordering::Relaxed);
    }
    if packet.starts_with(&[0x81, 0x02]) {
        STREAMING.store(true, Ordering::Relaxed);
        let mut out = vec![0xc1, 0x03, 0x00];
        out.extend([0xc2, 0x0e, 0x00, 0x00]);
        out.extend([0u8; 10]);
        return out;
    }
    Vec::new()
}

/// The panel streams, between the start and stop commands.
static STREAMING: AtomicBool = AtomicBool::new(false);

/// The panel's coordinates packet: the pointer's place over the screen (14 bits each axis) and
/// whether it touches.
fn panel_stream() -> Vec<u8> {
    if !STREAMING.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let stick = wal_payload_common::input(0);
    let touched = stick.pressed(button::B4);
    // the pointer's place in the game's 5:4 picture, fitted in the screen (pillarboxed)
    let (x0, y0, w, h) = picture();
    let to14 = |v: i16, o: f32, size: f32| {
        let f = (((v as f32 + 32768.0) / 65535.0 - o) / size).clamp(0.0, 1.0);
        (f * 16383.0) as u16
    };
    let (x, y) = (to14(stick.axis(Axis::RightX), x0, w), to14(stick.axis(Axis::RightY), y0, h));
    vec![0x82, touched as u8, 0, 0, (x & 0x7f) as u8, (x >> 7) as u8, (y & 0x7f) as u8, (y >> 7) as u8]
}

const CAMERA_W: i32 = 640;
const CAMERA_H: i32 = 480;

extern "C" fn camera_init() {}

extern "C" fn camera_open(_rate: f64) -> u32 {
    log!("ygo: camera opened");
    0
}

extern "C" fn camera_ok() -> u32 {
    0
}

extern "C" fn camera_count() -> i32 {
    1
}

extern "C" fn camera_buffer_size(camera: i32) -> i32 {
    if camera == 0 { CAMERA_W * CAMERA_H * 3 } else { 0 }
}

/// A camera picture: blank (no card under the camera).
extern "C" fn camera_image(camera: i32, out: *mut u8, _wait: bool) -> u32 {
    if camera != 0 || out.is_null() {
        return 1;
    }
    unsafe { std::ptr::write_bytes(out, 0, (CAMERA_W * CAMERA_H * 3) as usize) };
    0
}

/// The game's 1280x1024 picture on the screen, in fractions of it: its aspect fitted.
fn picture() -> (f32, f32, f32, f32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) };
    let aspect = 1280.0 / 1024.0;
    if sw <= 0.0 || sh <= 0.0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    if sw / sh > aspect {
        let w = sh * aspect / sw;
        ((1.0 - w) / 2.0, 0.0, w, 1.0)
    } else {
        let h = sw / aspect / sh;
        (0.0, (1.0 - h) / 2.0, 1.0, h)
    }
}

/// Puts back libpenteltp's observer update jumps (`CPentelTPObserver::update`, rva 0x2570): to
/// the release edge when the new packet is untouched (rva 0x25db), past the touch callback
/// then (0x2629). Another build: left as is.
fn restore_edges(panel: usize) {
    const FIXES: [(usize, [u8; 6], [u8; 2]); 2] = [
        (0x25d9, [0x85, 0xc0, 0x90, 0x90, 0x8b, 0x4c], [0x74, 0x24]),
        (0x2627, [0x85, 0xc0, 0x90, 0x90, 0x8b, 0x16], [0x74, 0x07]),
    ];
    if panel == 0 {
        return;
    }
    for (rva, expected, _) in FIXES {
        if unsafe { std::slice::from_raw_parts((panel + rva) as *const u8, 6) } != expected {
            log!("ygo: libpenteltp's update differs, left as is");
            return;
        }
    }
    for (rva, _, jump) in FIXES {
        let at = (panel + rva + 2) as *mut u8;
        let mut old = 0;
        unsafe {
            VirtualProtect(at.cast(), 2, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(jump.as_ptr(), at, 2);
            VirtualProtect(at.cast(), 2, old, &mut old);
        }
    }
    log!("ygo: libpenteltp's touch edges restored");
}

/// The game's calibration readers (`game.exe`): its saved values (rva 0x1b9b00: both targets'
/// raw touches, then their places on the screen) and whether it is calibrated (rva 0x1b9bf0).
const CALIBRATION_VALUES: usize = 0x1b9b00;
const CALIBRATED: usize = 0x1b9bf0;
/// Both readers' first instructions (`sub esp, 0x48; push esi; push edi`), moved to their
/// trampolines.
const READER_HEAD: [u8; 5] = [0x83, 0xec, 0x48, 0x56, 0x57];

static VALUES_TRAMPOLINE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CALIBRATED_TRAMPOLINE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The calibration of the targets (64, 80) and (1216, 944) touched dead on: 14-bit raw
/// touches over the 1280x1024 picture.
const EXACT_CALIBRATION: [i32; 8] = [64 * 16383 / 1280, 80 * 16383 / 1024, 1216 * 16383 / 1280, 944 * 16383 / 1024, 64, 80, 1216, 944];

fn saved_calibrated() -> bool {
    let original: extern "C" fn() -> bool = unsafe { std::mem::transmute(CALIBRATED_TRAMPOLINE.load(Ordering::Relaxed)) };
    original()
}

extern "C" fn calibration_values(out: *mut [i32; 8]) {
    let original: extern "C" fn(*mut [i32; 8]) = unsafe { std::mem::transmute(VALUES_TRAMPOLINE.load(Ordering::Relaxed)) };
    original(out);
    if !saved_calibrated() && !out.is_null() {
        unsafe { *out = EXACT_CALIBRATION };
    }
}

extern "C" fn calibrated() -> bool {
    true
}

/// Jumps from both calibration readers to ours (the originals kept in trampolines).
fn default_calibration() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let readers = [(CALIBRATION_VALUES, calibration_values as *const () as usize, &VALUES_TRAMPOLINE), (CALIBRATED, calibrated as *const () as usize, &CALIBRATED_TRAMPOLINE)];
    if readers.iter().any(|&(rva, _, _)| unsafe { std::slice::from_raw_parts((base + rva) as *const u8, 5) } != READER_HEAD) {
        log!("ygo: calibration readers differ, the test menu's calibration is needed");
        return;
    }
    let stubs = unsafe { VirtualAlloc(std::ptr::null(), 32, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as usize;
    if stubs == 0 {
        return;
    }
    for (i, &(rva, ours, trampoline)) in readers.iter().enumerate() {
        let at = base + rva;
        // the moved head, then a jump back past it
        let stub = stubs + i * 16;
        let mut code = READER_HEAD.to_vec();
        code.push(0xe9);
        code.extend_from_slice(&((at + 5).wrapping_sub(stub + 10) as i32).to_le_bytes());
        unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), stub as *mut u8, code.len()) };
        trampoline.store(stub, Ordering::Relaxed);
        let mut jump = vec![0xe9];
        jump.extend_from_slice(&ours.wrapping_sub(at + 5).to_le_bytes()[..4]);
        let mut old = 0;
        unsafe {
            VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, 5);
            VirtualProtect(at as *const _, 5, old, &mut old);
        }
    }
    log!("ygo: touch calibrated by default");
}
