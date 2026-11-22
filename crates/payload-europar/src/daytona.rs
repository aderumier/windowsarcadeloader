//! Daytona Championship USA (both editions), linked across PCs (`WAL_LINK`, payload-common
//! `link.rs`). The game finds its cabinets on 192.168.1.0/24, written in its code: its address is
//! the last byte of `gethostbyname(gethostname())`, cabinet 1 (`CabinetID`, `settings.rs`) the
//! main PC, which tries 192.168.1.1 to .8 on TCP 26000; the race then goes between those addresses
//! (a heart beat, the ranking on TCP 5888, a broadcast to 192.168.1.255): 192.168.1.k is cabinet
//! k's PC (`link::virtual_net`).
//!
//! The link also sends each player's camera pictures to the others (its OpenCV manager): without
//! a camera each cabinet drops the other right after "commLink OK". The profile keeps Wine's
//! video capture away from it (its graph corrupted the game's heap), so linked, its camera is an
//! emulated one: `cv::VideoCapture::VideoCapture(int)` (OpenCV 2.4 linked in the program, the
//! known builds checked) makes an object of the payload's, open, its frames a black 320x240 picture.

use wal_payload_common::{link, log};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The builds: the executable's time stamp, the constructor's RVA, its first bytes.
const BUILDS: [(u32, usize, [u8; 6], &str); 2] = [
    (0x5A3B_3168, 0x642DF0, [0x55, 0x8B, 0xEC, 0x6A, 0xFF, 0x68], "Daytona Championship USA"),
    (0x5C7F_7953, 0x6484F0, [0x55, 0x8B, 0xEC, 0x6A, 0xFF, 0x68], "Daytona Championship USA New Season Edition"),
];

const WIDTH: i32 = 320;
const HEIGHT: i32 = 240;
static FRAME: [u8; (WIDTH * HEIGHT * 3) as usize] = [0; (WIDTH * HEIGHT * 3) as usize];

/// `cv::Mat` of OpenCV 2.4, 32-bit.
#[repr(C)]
struct Mat {
    flags: i32,
    dims: i32,
    rows: i32,
    cols: i32,
    data: *const u8,
    refcount: *mut i32,
    datastart: *const u8,
    dataend: *const u8,
    datalimit: *const u8,
    allocator: usize,
    sizep: *mut i32,
    stepp: *mut usize,
    stepbuf: [usize; 2],
}

/// The game's frame made the black picture (no reference count: nothing for the game to free).
unsafe fn frame(m: *mut Mat) {
    if m.is_null() {
        return;
    }
    unsafe {
        let m = &mut *m;
        // Mat's magic, continuous, CV_8UC3
        m.flags = 0x42FF_0000 | 0x4000 | 16;
        m.dims = 2;
        m.rows = HEIGHT;
        m.cols = WIDTH;
        m.data = FRAME.as_ptr();
        m.datastart = FRAME.as_ptr();
        m.dataend = FRAME.as_ptr().add(FRAME.len());
        m.datalimit = m.dataend;
        m.refcount = std::ptr::null_mut();
        m.allocator = 0;
        m.sizep = &mut m.rows;
        m.stepp = m.stepbuf.as_mut_ptr();
        m.stepbuf = [(WIDTH * 3) as usize, 3];
    }
}

// cv::VideoCapture's virtual functions in MSVC's order (thiscall: fastcall, edx unused)
unsafe extern "fastcall" fn destroy(this: usize, _: usize, _flags: i32) -> usize {
    this
}
unsafe extern "fastcall" fn open(_: usize, _: usize, _what: usize) -> bool {
    true
}
unsafe extern "fastcall" fn yes(_: usize, _: usize) -> bool {
    true
}
unsafe extern "fastcall" fn release(_: usize, _: usize) {}
unsafe extern "fastcall" fn retrieve(_: usize, _: usize, m: *mut Mat, _channel: i32) -> bool {
    unsafe { frame(m) };
    true
}
unsafe extern "fastcall" fn take(this: usize, _: usize, m: *mut Mat) -> usize {
    unsafe { frame(m) };
    this
}
unsafe extern "fastcall" fn read(_: usize, _: usize, m: *mut Mat) -> bool {
    unsafe { frame(m) };
    true
}
unsafe extern "fastcall" fn set(_: usize, _: usize, _prop: i32, _value: f64) -> bool {
    true
}
unsafe extern "fastcall" fn get(_: usize, _: usize, prop: i32) -> f64 {
    match prop {
        3 => WIDTH as f64,
        4 => HEIGHT as f64,
        5 => 20.0,
        _ => 0.0,
    }
}

struct Vtable([usize; 11]);
static VTABLE: std::sync::OnceLock<Vtable> = std::sync::OnceLock::new();

/// `cv::VideoCapture::VideoCapture(int device)`: the emulated camera.
unsafe extern "fastcall" fn camera(this: *mut usize, _: usize, device: i32) -> *mut usize {
    let v = VTABLE.get_or_init(|| {
        Vtable([
            destroy as *const () as usize,
            open as *const () as usize,
            open as *const () as usize,
            yes as *const () as usize,
            release as *const () as usize,
            yes as *const () as usize,
            retrieve as *const () as usize,
            take as *const () as usize,
            read as *const () as usize,
            set as *const () as usize,
            get as *const () as usize,
        ])
    });
    unsafe {
        *this = v.0.as_ptr() as usize;
        *this.add(1) = 0;
        *this.add(2) = 0;
    }
    log!("daytona: the game's camera {device}: the emulated one (black, 320x240)");
    this
}

pub(crate) fn init() {
    if link::init() == 0 {
        return;
    }
    let base = wal_payload_common::iat::game_module();
    let stamp = unsafe {
        let pe = *((base + 0x3C) as *const u32) as usize;
        *((base + pe + 8) as *const u32)
    };
    let Some(&(_, rva, was, name)) = BUILDS.iter().find(|b| b.0 == stamp) else { return };
    let at = base + rva;
    if !wal_payload_common::crash::readable(at, was.len()) || unsafe { std::slice::from_raw_parts(at as *const u8, was.len()) } != was {
        return log!("daytona: {name}: cv::VideoCapture(int) not where expected: no emulated camera");
    }
    link::virtual_net([192, 168, 1], 1);
    unsafe {
        let mut old = 0;
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        *(at as *mut u8) = 0xE9;
        std::ptr::write_unaligned((at + 1) as *mut i32, (camera as *const () as usize as i32).wrapping_sub((at + 5) as i32));
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
    log!("daytona: {name} linked: cabinet {} at 192.168.1.{0}, its camera emulated", link::cabinet());
}
