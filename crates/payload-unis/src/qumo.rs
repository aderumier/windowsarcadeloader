//! The UNIS "qumo" gun cabinet (Night Hunter: After Dark Chapter II, Unity 5.5, Mono, 64-bit):
//! `WAL_UNIS_BOARD=qumo`.
//!
//! The cabinet: a USB HID board (STMicroelectronics 0483:5750, version 0200) for both guns, the
//! coins and the operator's buttons, behind a `Sense4` dongle. The game (`USB_HID.dll`, Mono)
//! finds the board with `SetupDi*` and `HidD_GetAttributes`, then reads its input reports and
//! writes its output reports (the guns' recoil, the lights) with overlapped `ReadFile` /
//! `WriteFile` on two handles of its own threads. Its `DllImport`s are resolved to the
//! payload's (`payload-common`'s `mono_imports.rs`): one device, its path's handles the
//! payload's, every other handle the real functions'.
//!
//! - **Input report** (16 bytes after the report id, answered to each PC report, at most
//!   every 50 ms without one): `AA`, byte 1 the operator's buttons (bit 2 test, 1 service,
//!   0 reset: the operator menu's selection and move), byte 2 each gun's buttons (P1 / P2 bits: 7 / 6 coin, 5 / 4 big button,
//!   3 / 2 trigger and start, 1 / 0 emergency stop), byte 3 bits 7 / 6 the change of
//!   weapon, then for each gun three bytes (P1 5..7, P2 8..10): x's low byte, y's low byte,
//!   x's high nibble (bits 4..7) and y's (bits 0..3). The game counts a coin on each press.
//! - **Aim.** 12 bits, 0..4095 across the picture from its bottom left (y up); the game maps it through
//!   its calibration (`Assets/config/game/excel_data/jiaoqiang.csv`: the profile's, 0..4095,
//!   the aim unchanged). The game draws its own gun marks.
//! - **Dongle.** `Sense4`: every call succeeds (Dino Invasion's, `shunxiang.rs`).
//!
//! - **Free play.** `WAL_UNIS_FREEPLAY`: the operator's setting in the game's saved settings.
//!
//! Natives: `trigger big change coin stop` (each gun), `test service reset` (the operator's).

use std::ffi::c_char;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

const PLAYERS: usize = 2;
const VENDOR: u16 = 0x0483;
const PRODUCT: u16 = 0x5750;
const VERSION: u16 = 0x0200;

/// The device's interface path (UTF-16 to `SetupDi*`, ANSI back to `CreateFile`).
const PATH: &str = r"\\?\hid#vid_0483&pid_5750#wal-qumo#{4d1e55b2-f16f-11cf-88cb-001111000030}";
/// The device list's handle and the device's file handles: far from Wine's (small multiples
/// of 4).
const DEVICE_LIST: usize = 0x5A11_0000;
const HANDLE_BASE: usize = 0x5A12_0000;
const REPORT: usize = 16;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Big,
    Change,
    Coin,
    Stop,
    /// An operator's button: its bit of byte 1.
    Operator(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("big", Native::Big),
    ("change", Native::Change),
    ("coin", Native::Coin),
    ("stop", Native::Stop),
    ("test", Native::Operator(2)),
    ("service", Native::Operator(1)),
    ("reset", Native::Operator(0)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    // the trigger starts a game (the board's start and trigger are one bit)
    ("start", "trigger"),
    ("b2", "big"),
    ("b3", "change"),
    ("coin", "coin"),
    // the operator menu: test opens it, reset moves, service selects
    ("test", "test"),
    ("service", "service"),
    ("down", "reset"),
    ("up", "reset"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// Each gun's last place on the picture (kept while it points off it), 0..4095.
static AIM: Mutex<[(u16, u16); PLAYERS]> = Mutex::new([(2048, 2048); PLAYERS]);

/// The board's input report.
fn report() -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    r[0] = 0xAA;
    let mut aim = AIM.lock().unwrap();
    for (player, aim) in aim.iter_mut().enumerate() {
        let shift = player as u8;
        map().for_each_pressed(&input(player), |n| match n {
            Native::Coin => r[2] |= 0x80 >> shift,
            Native::Big => r[2] |= 0x20 >> shift,
            Native::Trigger => r[2] |= 0x08 >> shift,
            Native::Stop => r[2] |= 0x02 >> shift,
            Native::Change => r[3] |= 0x80 >> shift,
            Native::Operator(bit) if player == 0 => r[1] |= 1 << bit,
            Native::Operator(_) => {}
        });
        if let Some((x, y)) = gun::gun_of_player_in_picture(player) {
            // y up
            *aim = ((x * 4095.0) as u16, ((1.0 - y) * 4095.0) as u16);
        }
        let at = 5 + 3 * player;
        r[at] = aim.0 as u8;
        r[at + 1] = aim.1 as u8;
        r[at + 2] = ((aim.0 >> 8) as u8) << 4 | (aim.1 >> 8) as u8;
    }
    r
}

/// The PC's reports since the last input report (the board answers each).
static WRITES: Mutex<u32> = Mutex::new(0);
static WRITTEN: Condvar = Condvar::new();
static FIRST: AtomicBool = AtomicBool::new(true);
static HANDLES: AtomicUsize = AtomicUsize::new(0);

fn ours(handle: usize) -> bool {
    (HANDLE_BASE..HANDLE_BASE + 0x1000).contains(&handle)
}

/// A real `kernel32` / `hid` function, for the handles not the device's.
fn real(module: &str, name: &str) -> usize {
    let module: Vec<u16> = module.encode_utf16().chain([0]).collect();
    let name = format!("{name}\0");
    let m = unsafe { GetModuleHandleW(module.as_ptr()) };
    unsafe { GetProcAddress(m, name.as_ptr()) }.map_or(0, |f| f as usize)
}

macro_rules! real {
    ($module:literal, $name:literal, $t:ty) => {{
        static F: OnceLock<usize> = OnceLock::new();
        let f = *F.get_or_init(|| real($module, $name));
        unsafe { std::mem::transmute::<usize, $t>(f) }
    }};
}

/// `OVERLAPPED`: `Internal`, `InternalHigh` (the bytes transferred), offsets, `hEvent`.
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u64,
    event: usize,
}

fn complete(overlapped: *mut Overlapped, len: u32) {
    if overlapped.is_null() {
        return;
    }
    let set_event = real!("kernel32.dll", "SetEvent", unsafe extern "system" fn(usize) -> i32);
    unsafe {
        (*overlapped).internal = 0;
        (*overlapped).internal_high = len as usize;
        if (*overlapped).event != 0 {
            set_event((*overlapped).event);
        }
    }
}

// setupapi: one HID interface, the board's

extern "system" fn get_class_devs(_guid: *const u8, _enumerator: usize, _hwnd: usize, _flags: u32) -> usize {
    DEVICE_LIST
}

/// `SetupDiEnumDeviceInterfaces(set, devinfo, guid, index, data)`: `data` is `cbSize`, the
/// interface class's GUID, flags, reserved.
extern "system" fn enum_interfaces(_set: usize, _dev: usize, guid: *const u8, index: i32, data: *mut u8) -> i32 {
    if index != 0 || data.is_null() {
        unsafe { windows_sys::Win32::Foundation::SetLastError(259) };
        return 0;
    }
    unsafe {
        if !guid.is_null() {
            std::ptr::copy_nonoverlapping(guid, data.add(4), 16);
        }
        *(data.add(20) as *mut u32) = 1;
    }
    1
}

/// `SetupDiGetDeviceInterfaceDetailW(set, data, detail, size, required, devinfo)`: the size
/// asked first, then the path after `cbSize`.
extern "system" fn interface_detail(_set: usize, _data: *const u8, detail: *mut u8, size: u32, required: *mut u32, _devinfo: *mut u8) -> i32 {
    let path: Vec<u16> = PATH.encode_utf16().chain([0]).collect();
    let need = 4 + 2 * path.len() as u32;
    if !required.is_null() {
        unsafe { *required = need };
    }
    if detail.is_null() || size < need {
        unsafe { windows_sys::Win32::Foundation::SetLastError(122) };
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(path.as_ptr().cast::<u8>(), detail.add(4), 2 * path.len()) };
    1
}

extern "system" fn destroy_info_list(_set: usize) -> i32 {
    1
}

// hid: the board's attributes

/// `HIDD_ATTRIBUTES`: size, vendor, product, version.
extern "system" fn get_attributes(handle: usize, attr: *mut u16) -> u8 {
    if !ours(handle) {
        let f = real!("hid.dll", "HidD_GetAttributes", unsafe extern "system" fn(usize, *mut u16) -> u8);
        return unsafe { f(handle, attr) };
    }
    unsafe {
        *(attr as *mut u32) = 12;
        *attr.add(2) = VENDOR;
        *attr.add(3) = PRODUCT;
        *attr.add(4) = VERSION;
    }
    1
}

// kernel32: the device's handles

extern "system" fn create_file(name: *const c_char, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    let path = if name.is_null() { "" } else { unsafe { std::ffi::CStr::from_ptr(name) }.to_str().unwrap_or("") };
    if !path.contains("wal-qumo") {
        let f = real!("kernel32.dll", "CreateFileA", unsafe extern "system" fn(*const c_char, u32, u32, usize, u32, u32, usize) -> usize);
        return unsafe { f(name, access, share, sa, disp, flags, template) };
    }
    if access != 0 {
        log!("qumo: the board opened (access {access:#x})");
    }
    HANDLE_BASE + 4 * (HANDLES.fetch_add(1, Ordering::Relaxed) % 0x400)
}

/// An input report, after the PC's next report (or 50 ms): `[report id 0, 16 bytes]`.
extern "system" fn read_file(handle: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: *mut Overlapped) -> i32 {
    if !ours(handle) {
        let f = real!("kernel32.dll", "ReadFile", unsafe extern "system" fn(usize, *mut u8, u32, *mut u32, *mut Overlapped) -> i32);
        return unsafe { f(handle, buf, len, read, overlapped) };
    }
    {
        let writes = WRITES.lock().unwrap();
        let (mut writes, _) = WRITTEN.wait_timeout_while(writes, Duration::from_millis(50), |w| *w == 0).unwrap();
        *writes = 0;
    }
    let r = report();
    let n = (REPORT + 1).min(len as usize);
    unsafe {
        *buf = 0;
        std::ptr::copy_nonoverlapping(r.as_ptr(), buf.add(1), n - 1);
        if !read.is_null() {
            *read = n as u32;
        }
    }
    if FIRST.swap(false, Ordering::Relaxed) {
        log!("qumo: first input report to the game");
    }
    complete(overlapped, n as u32);
    1
}

/// The PC's report (recoil, lights): accepted.
extern "system" fn write_file(handle: usize, buf: *const u8, len: u32, written: *mut u32, overlapped: *mut Overlapped) -> i32 {
    if !ours(handle) {
        let f = real!("kernel32.dll", "WriteFile", unsafe extern "system" fn(usize, *const u8, u32, *mut u32, *mut Overlapped) -> i32);
        return unsafe { f(handle, buf, len, written, overlapped) };
    }
    *WRITES.lock().unwrap() += 1;
    WRITTEN.notify_all();
    if !written.is_null() {
        unsafe { *written = len };
    }
    complete(overlapped, len);
    1
}

extern "system" fn overlapped_result(handle: usize, overlapped: *mut Overlapped, n: *mut u32, wait: i32) -> i32 {
    if !ours(handle) {
        let f = real!("kernel32.dll", "GetOverlappedResult", unsafe extern "system" fn(usize, *mut Overlapped, *mut u32, i32) -> i32);
        return unsafe { f(handle, overlapped, n, wait) };
    }
    if !n.is_null() && !overlapped.is_null() {
        unsafe { *n = (*overlapped).internal_high as u32 };
    }
    1
}

extern "system" fn close_handle(handle: usize) -> i32 {
    if ours(handle) {
        return 1;
    }
    let f = real!("kernel32.dll", "CloseHandle", unsafe extern "system" fn(usize) -> i32);
    unsafe { f(handle) }
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    let f = |f: usize| Some(f);
    match (module, name) {
        ("setupapi.dll", n) if n.starts_with("SetupDiGetClassDevs") => f(get_class_devs as *const () as usize),
        ("setupapi.dll", "SetupDiEnumDeviceInterfaces") => f(enum_interfaces as *const () as usize),
        ("setupapi.dll", n) if n.starts_with("SetupDiGetDeviceInterfaceDetail") => f(interface_detail as *const () as usize),
        ("setupapi.dll", "SetupDiDestroyDeviceInfoList") => f(destroy_info_list as *const () as usize),
        ("hid.dll", "HidD_GetAttributes") => f(get_attributes as *const () as usize),
        ("kernel32.dll", "CreateFile" | "CreateFileA") => f(create_file as *const () as usize),
        ("kernel32.dll", "ReadFile") => f(read_file as *const () as usize),
        ("kernel32.dll", "WriteFile") => f(write_file as *const () as usize),
        ("kernel32.dll", "GetOverlappedResult") => f(overlapped_result as *const () as usize),
        ("kernel32.dll", "CloseHandle") => f(close_handle as *const () as usize),
        _ => crate::shunxiang::dongles(name, module),
    }
}

/// The operator's free play setting (`WAL_UNIS_FREEPLAY`: "1" on, "0" off, unset: the game's),
/// set in its saved settings (`Free_Play` in `Assets/config/game/excel_data/data_info.csv`, the
/// bookkeeping kept) before the game reads them.
fn free_play() {
    let Some(on) = wal_payload_common::free_play("WAL_UNIS_FREEPLAY") else { return };
    let Some(path) = std::env::current_exe().ok().map(|e| e.with_file_name("Assets/config/game/excel_data/data_info.csv")) else { return };
    let Ok(data) = std::fs::read_to_string(&path) else { return };
    let Some(updated) = with_free_play(&data, on) else { return };
    if updated != data && std::fs::write(&path, updated).is_ok() {
        log!("qumo: free play {}", if on { "on" } else { "off" });
    }
}

/// The settings with `Free_Play` set, or None (no such line).
fn with_free_play(data: &str, on: bool) -> Option<String> {
    let mut found = false;
    let lines: Vec<String> = data
        .split('\n')
        .map(|l| match l.strip_prefix("Free_Play,") {
            Some(rest) => {
                found = true;
                let cr = if rest.ends_with('\r') { "\r" } else { "" };
                format!("Free_Play,{}{cr}", on as u8)
            }
            None => l.to_string(),
        })
        .collect();
    found.then(|| lines.join("\n"))
}

pub fn init() {
    free_play();
    mono_imports::hook(resolve);
    crate::nighthunter_mission4::init();
    log!("qumo: HID board {VENDOR:04x}:{PRODUCT:04x}, dongle");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_ours() {
        assert!(PATH.contains("wal-qumo"));
        assert!(ours(HANDLE_BASE) && !ours(0x100));
    }

    #[test]
    fn free_play_line() {
        assert_eq!(with_free_play("Difficulty,2\nFree_Play,0\nVolume,8", true).as_deref(), Some("Difficulty,2\nFree_Play,1\nVolume,8"));
        assert_eq!(with_free_play("Free_Play,1\r\n", false).as_deref(), Some("Free_Play,0\r\n"));
        assert!(with_free_play("Volume,8", true).is_none());
    }

    #[test]
    fn overlapped_layout() {
        assert_eq!(std::mem::size_of::<Overlapped>(), 32);
    }
}
