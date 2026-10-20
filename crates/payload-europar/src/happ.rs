//! The Happ UGCI USB board (`USB\VID_078B&PID_0010`), a HID device with several top-level
//! collections: wheel, pedals, buttons, coin counter...
//!
//! The game (`0x4de1f0`) and the GameShell (`0x4030c1`, the same "HappAPI" code) enumerate the
//! HID class with SetupAPI, open each interface with `CreateFileA`, keep those whose
//! `HidD_GetAttributes` reads vendor 0x078b product 0x0010, and name each one by its
//! collection (`HidP_GetCaps`: usage page / usage, a table): 1 / 4 "JOYSTICK", 0x91 / 2 "COIN
//! DROP" (also 0x91 / 3 "WATCHDOG", 1 / 0x80 "SYSTEM", ..., not emulated). Each is read with
//! `ReadFile`:
//!
//! JOYSTICK, 8 bytes (the game's thread `0x4de030`, the shell's `0x40365a`):
//!
//! | byte | content |
//! |---|---|
//! | 1 | wheel, 0 (left) ..= 255 (right) |
//! | 3 | gas, 0 (released) ..= 255 |
//! | 5 | brake, 0 (released) ..= 255 |
//! | 6 | buttons: 0x01 shift up, 0x02 shift down, 0x04 start, 0x08 view change, 0x10 TEST, 0x20 SERVICE |
//!
//! (the ranges are `data/calibration.ini`'s; the game drops a report whose byte 6 is 7. The
//! shell's button table at `0x431628`: keys 1-6 = bits 0x04 0x08 0x01 0x02 0x10 0x20; key 5
//! closes the game into the test menu, key 6 is a service credit.)
//!
//! COIN DROP, 4 bytes (the shell's coin thread `0x40377d`): bytes 1-2 a 16-bit coin counter,
//! each change a coin.
//!
//! Every call is answered by IAT hooks: the game sees the JOYSTICK collection, the shell both,
//! their reports built from player 1's virtual stick every 8 ms (the board's polling rate).
//!
//! `WAL_EUROPAR_BOARD=rally3`: SEGA Rally 3's board instead (`0x57b2f0`: the device whose
//! attributes read vendor 0x1043 product 0, any collection), 15-byte reports read by
//! `0x57b640`: byte 1 wheel (and 2), 3 brake, 4 gas, 6 buttons 0x01 start, 0x02 handbrake,
//! 0x04 view change, 0x08 shift up, 0x10 shift down.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, log, mapping::ButtonMap};
use wal_protocol::Axis;

/// The interface paths the game opens (`CreateFileA`), `#` replaced by the collection index.
const DEVICE_PATH: &str = "\\\\?\\hid#vid_078b&pid_0010&col0#wal#{4d1e55b2-f16f-11cf-88cb-001111000030}";
/// Sentinel handles: the device information set, the opened collections (+ index).
const DEVINFO: usize = 0x5741_4C00;
const HANDLE: usize = 0x5741_4C10;
/// The preparsed data the game passes back to `HidP_GetCaps` (one word per collection).
static PREPARSED: [u32; 2] = [0x5741_4C50; 2];

/// A top-level collection: usage page, usage, report length.
struct Collection {
    page: u16,
    usage: u16,
    len: u16,
}
const JOYSTICK: Collection = Collection { page: 1, usage: 4, len: 8 };
const COIN_DROP: Collection = Collection { page: 0x91, usage: 2, len: 4 };

/// The collections shown: the game's JOYSTICK, plus COIN DROP in the shell.
static COUNT: AtomicUsize = AtomicUsize::new(1);
/// Coins dropped (the COIN DROP counter).
static COINS: AtomicU32 = AtomicU32::new(0);

fn collection(index: usize) -> &'static Collection {
    if index == 0 { &JOYSTICK } else { &COIN_DROP }
}

fn path(index: usize) -> String {
    DEVICE_PATH.replace("col0", &format!("col{index}"))
}

/// The opened collection of a handle.
fn opened(handle: usize) -> Option<usize> {
    (HANDLE..HANDLE + COUNT.load(Ordering::Relaxed)).contains(&handle).then(|| handle - HANDLE)
}

const HIDP_STATUS_SUCCESS: u32 = 0x0011_0000;
const ERROR_NO_MORE_ITEMS: u32 = 0x103;

/// SEGA Rally 3's board (`WAL_EUROPAR_BOARD=rally3`).
static RALLY3: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn rally3() -> bool {
    RALLY3.load(Ordering::Relaxed)
}

/// SEGA Rally 3's natives (report byte 6) and default map.
const RALLY3_NATIVES: &[(&str, u8)] =
    &[("start", 0x01), ("handbrake", 0x02), ("view", 0x04), ("shift_up", 0x08), ("shift_down", 0x10)];
const RALLY3_MAP: &[(&str, &str)] =
    &[("start", "start"), ("b1", "shift_up"), ("b2", "shift_down"), ("b3", "view"), ("b4", "handbrake")];

/// Native inputs: the button bits of report byte 6.
const NATIVES: &[(&str, u8)] =
    &[("shift_up", 0x01), ("shift_down", 0x02), ("start", 0x04), ("view", 0x08), ("test", 0x10), ("service", 0x20)];

/// Virtual stick -> board, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("b1", "shift_up"),
    ("b2", "shift_down"),
    ("b3", "view"),
    ("test", "test"),
    ("service", "service"),
];

static MAP: OnceLock<ButtonMap<u8>> = OnceLock::new();
/// A TEST press of the payload's own (the shell's way into its test menu).
static TEST_PRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Presses TEST after `after`, for `hold`.
pub(crate) fn press_test(after: Duration, hold: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(after);
        TEST_PRESSED.store(true, Ordering::Relaxed);
        std::thread::sleep(hold);
        TEST_PRESSED.store(false, Ordering::Relaxed);
    });
}
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(u32::MAX);

static ORIG_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_READ_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLOSE_HANDLE: AtomicUsize = AtomicUsize::new(0);

fn map() -> &'static ButtonMap<u8> {
    MAP.get_or_init(|| if rally3() { ButtonMap::new(RALLY3_NATIVES, RALLY3_MAP) } else { ButtonMap::new(NATIVES, DEFAULT_MAP) })
}

/// The COIN DROP report: the coin counter.
fn coin_report() -> [u8; 16] {
    let coins = COINS.load(Ordering::Relaxed) as u16;
    let mut r = [0u8; 16];
    r[1..3].copy_from_slice(&coins.to_le_bytes());
    r
}

/// The JOYSTICK report from player 1 (8 bytes; SEGA Rally 3's board: 15).
fn report() -> [u8; 16] {
    let stick = wal_payload_common::input(0);
    let mut buttons = 0u8;
    map().for_each_pressed(&stick, |b| buttons |= b);
    if TEST_PRESSED.load(Ordering::Relaxed) {
        buttons |= 0x10;
    }
    if LAST_BUTTONS.swap(buttons as u32, Ordering::Relaxed) != buttons as u32 {
        log!("happ: buttons {buttons:#04x}");
    }
    let mut r = [0u8; 16];
    r[1] = stick.axis_u8(Axis::LeftX);
    if rally3() {
        r[2] = r[1];
        r[3] = stick.axis_u8(Axis::Brake);
        r[4] = stick.axis_u8(Axis::Accel);
    } else {
        r[2] = 0x80;
        r[3] = stick.axis_u8(Axis::Accel);
        r[5] = stick.axis_u8(Axis::Brake);
    }
    r[6] = buttons;
    r
}

// ── SetupAPI: one HID interface ───────────────────────────────────────────────────────────────

unsafe extern "system" fn get_class_devs(_guid: *const u8, _enumerator: *const i8, _hwnd: usize, flags: u32) -> usize {
    log!("happ: SetupDiGetClassDevsA(flags {flags:#x}): the board");
    DEVINFO
}

/// `SP_DEVICE_INTERFACE_DATA { cbSize, InterfaceClassGuid, Flags, Reserved }`: the collection
/// index + 1 kept in `Reserved` for the detail call.
unsafe extern "system" fn enum_interfaces(_set: usize, _dev: usize, _guid: *const u8, index: u32, data: *mut u8) -> i32 {
    if (index as usize) < COUNT.load(Ordering::Relaxed) {
        if !data.is_null() {
            unsafe { (data.add(24) as *mut usize).write_unaligned(index as usize + 1) };
        }
        return 1;
    }
    unsafe { windows_sys::Win32::Foundation::SetLastError(ERROR_NO_MORE_ITEMS) };
    0
}

/// `SetupDiGetDeviceInterfaceDetailA(set, data, detail, size, required, devinfo)`: the size
/// first (no buffer), then `cbSize` + the device path.
unsafe extern "system" fn interface_detail(_set: usize, data: *const u8, detail: *mut u8, size: u32, required: *mut u32, _devinfo: *mut u8) -> i32 {
    let index = if data.is_null() { 0 } else { unsafe { (data.add(24) as *const usize).read_unaligned() }.saturating_sub(1) };
    let path = path(index) + "\0";
    let needed = 4 + path.len();
    unsafe {
        if !required.is_null() {
            *required = needed as u32;
        }
        if detail.is_null() || (size as usize) < needed {
            windows_sys::Win32::Foundation::SetLastError(0x7a); // ERROR_INSUFFICIENT_BUFFER
            return 0;
        }
        std::ptr::copy_nonoverlapping(path.as_ptr(), detail.add(4), path.len());
    }
    1
}

unsafe extern "system" fn destroy_info_list(_set: usize) -> i32 {
    1
}

// ── HID ───────────────────────────────────────────────────────────────────────────────────────

/// `HIDD_ATTRIBUTES { Size, VendorID, ProductID, VersionNumber }`
unsafe extern "system" fn get_attributes(handle: usize, attr: *mut u8) -> u8 {
    if opened(handle).is_none() || attr.is_null() {
        return 0;
    }
    unsafe {
        let (vendor, product) = if rally3() { (0x1043, 0) } else { (0x078b, 0x0010) };
        (attr.add(4) as *mut u16).write_unaligned(vendor);
        (attr.add(6) as *mut u16).write_unaligned(product);
        (attr.add(8) as *mut u16).write_unaligned(0x0100);
    }
    1
}

unsafe extern "system" fn get_preparsed_data(handle: usize, out: *mut usize) -> u8 {
    let Some(index) = opened(handle) else { return 0 };
    if out.is_null() {
        return 0;
    }
    unsafe { *out = PREPARSED.as_ptr().add(index) as usize };
    1
}

/// `HIDP_CAPS` (64 bytes): the collection's usage page and usage, its report length.
unsafe extern "system" fn get_caps(preparsed: usize, caps: *mut u16) -> u32 {
    let index = preparsed.wrapping_sub(PREPARSED.as_ptr() as usize) / 4;
    if caps.is_null() || index >= PREPARSED.len() {
        return 0xc011_0001; // HIDP_STATUS_INVALID_PREPARSED_DATA
    }
    let c = collection(index);
    unsafe {
        std::ptr::write_bytes(caps as *mut u8, 0, 64);
        *caps = c.usage;
        *caps.add(1) = c.page;
        *caps.add(2) = if rally3() { 15 } else { c.len };
    }
    log!("happ: HidP_GetCaps: collection {:#x}/{:#x}", c.page, c.usage);
    HIDP_STATUS_SUCCESS
}

// ── Files: the device ─────────────────────────────────────────────────────────────────────────

type CreateFileA = unsafe extern "system" fn(*const i8, u32, u32, usize, u32, u32, usize) -> usize;
type ReadFile = unsafe extern "system" fn(usize, *mut u8, u32, *mut u32, usize) -> i32;
type CloseHandle = unsafe extern "system" fn(usize) -> i32;

unsafe extern "system" fn create_file(name: *const i8, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() {
        let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
        if let Some(index) = (0..COUNT.load(Ordering::Relaxed)).find(|&i| path(i).as_bytes() == n) {
            log!("happ: collection {index} opened");
            return HANDLE + index;
        }
    }
    let f: CreateFileA = unsafe { std::mem::transmute(ORIG_CREATE_FILE.load(Ordering::Relaxed)) };
    unsafe { f(name, access, share, sa, disp, flags, template) }
}

unsafe extern "system" fn read_file(handle: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32 {
    let Some(index) = opened(handle) else {
        let f: ReadFile = unsafe { std::mem::transmute(ORIG_READ_FILE.load(Ordering::Relaxed)) };
        return unsafe { f(handle, buf, len, read, overlapped) };
    };
    // the next interrupt report
    std::thread::sleep(Duration::from_millis(8));
    let r = if index == 0 { report() } else { coin_report() };
    let n = r.len().min(len as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(r.as_ptr(), buf, n);
        if !read.is_null() {
            *read = n as u32;
        }
    }
    1
}

unsafe extern "system" fn close_handle(handle: usize) -> i32 {
    if opened(handle).is_some() {
        log!("happ: collection closed");
        return 1;
    }
    let f: CloseHandle = unsafe { std::mem::transmute(ORIG_CLOSE_HANDLE.load(Ordering::Relaxed)) };
    unsafe { f(handle) }
}

/// `shell`: the GameShell (the COIN DROP collection too, its coins from player 1's coin).
pub(crate) fn init(shell: bool) {
    if std::env::var("WAL_EUROPAR_BOARD").is_ok_and(|v| v.trim().eq_ignore_ascii_case("rally3")) {
        RALLY3.store(true, Ordering::Relaxed);
        log!("happ: SEGA Rally 3's board");
    }
    map();
    if shell {
        COUNT.store(2, Ordering::Relaxed);
        std::thread::spawn(|| {
            let mut held = false;
            loop {
                let coin = wal_payload_common::input(0).pressed(wal_protocol::button::COIN);
                if coin && !held {
                    log!("happ: coin ({})", COINS.fetch_add(1, Ordering::Relaxed) + 1);
                }
                held = coin;
                std::thread::sleep(Duration::from_millis(5));
            }
        });
    }
    unsafe {
        let hooks: [(&str, &str, usize); 7] = [
            ("setupapi.dll", "SetupDiGetClassDevsA", get_class_devs as *const () as usize),
            ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize),
            ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA", interface_detail as *const () as usize),
            ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy_info_list as *const () as usize),
            ("hid.dll", "HidD_GetAttributes", get_attributes as *const () as usize),
            ("hid.dll", "HidD_GetPreparsedData", get_preparsed_data as *const () as usize),
            ("hid.dll", "HidP_GetCaps", get_caps as *const () as usize),
        ];
        for (dll, name, f) in hooks {
            if iat::hook(dll, name, f).is_none() {
                log!("happ: {dll}!{name} not imported");
            }
        }
        let files: [(&str, usize, &AtomicUsize); 3] = [
            ("CreateFileA", create_file as *const () as usize, &ORIG_CREATE_FILE),
            ("ReadFile", read_file as *const () as usize, &ORIG_READ_FILE),
            ("CloseHandle", close_handle as *const () as usize, &ORIG_CLOSE_HANDLE),
        ];
        for (name, f, orig) in files {
            if let Some(o) = iat::hook("kernel32.dll", name, f) {
                orig.store(o, Ordering::Relaxed);
            }
        }
    }
}
