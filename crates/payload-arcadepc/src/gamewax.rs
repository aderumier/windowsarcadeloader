//! Pigs Might Fly (Gamewax, 2009; `WAL_ARCADEPC_BOARD=gamewax`, x86 build), a redemption game of
//! the Sega Amusements Shell family, without the Shell:
//!
//! - **Board.** A HID device (`0x43eaa0`: vendor 0x1043 product 0, the board of SEGA Rally 3 and
//!   of the Shell; named "GAMEPAD" from its collection, usage 1 / 5). The game's thread
//!   (`0x43ef50`) reads 15-byte reports: bytes 6 / 7 the buttons (bit 0 start, 4 the lever,
//!   13 SERVICE), bytes 9 / 10 the coin counters (told to the Shell). Its output
//!   reports (lamps) are dropped.
//! - **Settings.** `GetPrivateProfileStringA` (`0x441850`): `[system]` from `.\game.ini`,
//!   `[GameSettings]` from `..\ShellData\GameSettings.ini`, the others from
//!   `..\ShellData\ShellData.ini`. `WAL_ARCADEPC_INI=<section>/<key>=<value>,...` overrides them
//!   (the Shell off, the portrait screen), the common `WAL_FREEPLAY` is `Credit/Freeplay`.
//! - **Without the Shell (`system/SHELLENABLE=0`).** The game needs a Shell window on its command
//!   line (`/h=`, the profile's dummy) and, with the setting off, sends it nothing. A SERVICE
//!   press is a credit (`0x43e720`: the coin here). Tickets: its free play path pays none
//!   (`0x43f238`, `0x43f420`), the profile's patch takes it for coins too; else it waits for the
//!   Shell's count, VENDING ERROR.
//!
//! Natives: `start lever coin`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, log, mapping::ButtonMap};

const DEVICE_PATH: &[u8] = b"\\\\?\\hid#vid_1043&pid_0000#wal#{4d1e55b2-f16f-11cf-88cb-001111000030}\0";
/// Sentinel handles: the device information set, the opened board.
const DEVINFO: usize = 0x5741_4C00;
const HANDLE: usize = 0x5741_4C10;
static PREPARSED: u32 = 0x5741_4C50;
const REPORT: usize = 15;
const OUTPUT_REPORT: u16 = 3;

const HIDP_STATUS_SUCCESS: u32 = 0x0011_0000;
const ERROR_NO_MORE_ITEMS: u32 = 0x103;
const ERROR_INSUFFICIENT_BUFFER: u32 = 0x7a;

const NATIVES: &[(&str, u16)] = &[("start", 0x0001), ("lever", 0x0010), ("coin", 0x2000)];
const DEFAULT_MAP: &[(&str, &str)] = &[("start", "start"), ("b1", "lever"), ("service", "coin"), ("coin", "coin")];

static MAP: OnceLock<ButtonMap<u16>> = OnceLock::new();
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(u32::MAX);

static ORIG_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_READ_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_WRITE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLOSE_HANDLE: AtomicUsize = AtomicUsize::new(0);
static ORIG_PROFILE_STRING: AtomicUsize = AtomicUsize::new(0);

fn map() -> &'static ButtonMap<u16> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

fn report() -> [u8; REPORT] {
    let mut buttons = 0u16;
    map().for_each_pressed(&wal_payload_common::input(0), |b| buttons |= b);
    if LAST_BUTTONS.swap(buttons as u32, Ordering::Relaxed) != buttons as u32 {
        log!("gamewax: buttons {buttons:#06x}");
    }
    let mut r = [0u8; REPORT];
    r[6..8].copy_from_slice(&buttons.to_le_bytes());
    r
}

// ── SetupAPI: one HID interface ───────────────────────────────────────────────────────────────

unsafe extern "system" fn get_class_devs(_guid: *const u8, _enumerator: *const i8, _hwnd: usize, _flags: u32) -> usize {
    DEVINFO
}

unsafe extern "system" fn enum_interfaces(_set: usize, _dev: usize, _guid: *const u8, index: u32, _data: *mut u8) -> i32 {
    if index == 0 {
        return 1;
    }
    unsafe { windows_sys::Win32::Foundation::SetLastError(ERROR_NO_MORE_ITEMS) };
    0
}

/// `SetupDiGetDeviceInterfaceDetailA(set, data, detail, size, required, devinfo)`: the size
/// first (no buffer), then `cbSize` + the device path.
unsafe extern "system" fn interface_detail(_set: usize, _data: *const u8, detail: *mut u8, size: u32, required: *mut u32, _devinfo: *mut u8) -> i32 {
    let needed = 4 + DEVICE_PATH.len();
    unsafe {
        if !required.is_null() {
            *required = needed as u32;
        }
        if detail.is_null() || (size as usize) < needed {
            windows_sys::Win32::Foundation::SetLastError(ERROR_INSUFFICIENT_BUFFER);
            return 0;
        }
        std::ptr::copy_nonoverlapping(DEVICE_PATH.as_ptr(), detail.add(4), DEVICE_PATH.len());
    }
    1
}

unsafe extern "system" fn destroy_info_list(_set: usize) -> i32 {
    1
}

// ── HID ───────────────────────────────────────────────────────────────────────────────────────

/// `HIDD_ATTRIBUTES { Size, VendorID, ProductID, VersionNumber }`
unsafe extern "system" fn get_attributes(handle: usize, attr: *mut u8) -> u8 {
    if handle != HANDLE || attr.is_null() {
        return 0;
    }
    unsafe {
        (attr.add(4) as *mut u16).write_unaligned(0x1043);
        (attr.add(6) as *mut u16).write_unaligned(0);
        (attr.add(8) as *mut u16).write_unaligned(0x0100);
    }
    1
}

unsafe extern "system" fn get_preparsed_data(handle: usize, out: *mut usize) -> u8 {
    if handle != HANDLE || out.is_null() {
        return 0;
    }
    unsafe { *out = &PREPARSED as *const u32 as usize };
    1
}

/// `HIDP_CAPS` (64 bytes): Usage, UsagePage, the input and output report lengths.
unsafe extern "system" fn get_caps(_preparsed: usize, caps: *mut u16) -> u32 {
    unsafe {
        std::ptr::write_bytes(caps as *mut u8, 0, 64);
        *caps = 5;
        *caps.add(1) = 1;
        *caps.add(2) = REPORT as u16;
        *caps.add(3) = OUTPUT_REPORT;
    }
    HIDP_STATUS_SUCCESS
}

// ── Files: the device ─────────────────────────────────────────────────────────────────────────

type CreateFileA = unsafe extern "system" fn(*const i8, u32, u32, usize, u32, u32, usize) -> usize;
type ReadWriteFile = unsafe extern "system" fn(usize, *mut u8, u32, *mut u32, usize) -> i32;
type CloseHandle = unsafe extern "system" fn(usize) -> i32;

unsafe extern "system" fn create_file(name: *const i8, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes_with_nul() == DEVICE_PATH {
        log!("gamewax: the board opened");
        return HANDLE;
    }
    let f: CreateFileA = unsafe { std::mem::transmute(ORIG_CREATE_FILE.load(Ordering::Relaxed)) };
    unsafe { f(name, access, share, sa, disp, flags, template) }
}

unsafe extern "system" fn read_file(handle: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32 {
    if handle != HANDLE {
        let f: ReadWriteFile = unsafe { std::mem::transmute(ORIG_READ_FILE.load(Ordering::Relaxed)) };
        return unsafe { f(handle, buf, len, read, overlapped) };
    }
    // the next interrupt report
    std::thread::sleep(Duration::from_millis(8));
    let r = report();
    let n = r.len().min(len as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(r.as_ptr(), buf, n);
        if !read.is_null() {
            *read = n as u32;
        }
    }
    1
}

unsafe extern "system" fn write_file(handle: usize, buf: *mut u8, len: u32, written: *mut u32, overlapped: usize) -> i32 {
    if handle != HANDLE {
        let f: ReadWriteFile = unsafe { std::mem::transmute(ORIG_WRITE_FILE.load(Ordering::Relaxed)) };
        return unsafe { f(handle, buf, len, written, overlapped) };
    }
    if !written.is_null() {
        unsafe { *written = len };
    }
    1
}

unsafe extern "system" fn close_handle(handle: usize) -> i32 {
    if handle == HANDLE {
        return 1;
    }
    let f: CloseHandle = unsafe { std::mem::transmute(ORIG_CLOSE_HANDLE.load(Ordering::Relaxed)) };
    unsafe { f(handle) }
}

// ── Settings ──────────────────────────────────────────────────────────────────────────────────

/// (section, key, value), from the profile.
fn overrides() -> &'static [(String, String, String)] {
    static LIST: OnceLock<Vec<(String, String, String)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut list: Vec<(String, String, String)> = std::env::var("WAL_ARCADEPC_INI")
            .unwrap_or_default()
            .split(',')
            .filter_map(|item| {
                let (name, value) = item.split_once('=')?;
                let (section, key) = name.trim().split_once('/')?;
                Some((section.trim().to_string(), key.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        if let Some(on) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
            list.push(("Credit".into(), "Freeplay".into(), if on { "1" } else { "0" }.into()));
        }
        list
    })
}

fn c_str(p: *const u8) -> String {
    if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() }
}

unsafe extern "system" fn get_profile_string(section: *const u8, key: *const u8, default: *const u8, out: *mut u8, size: u32, file: *const u8) -> u32 {
    let (s, k) = (c_str(section), c_str(key));
    if let Some((_, _, v)) = overrides().iter().find(|(os, ok, _)| os.eq_ignore_ascii_case(&s) && ok.eq_ignore_ascii_case(&k))
        && !out.is_null()
        && size > 0
    {
        let n = v.len().min(size as usize - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(v.as_ptr(), out, n);
            *out.add(n) = 0;
        }
        return n as u32;
    }
    let f: unsafe extern "system" fn(*const u8, *const u8, *const u8, *mut u8, u32, *const u8) -> u32 = unsafe { std::mem::transmute(ORIG_PROFILE_STRING.load(Ordering::Relaxed)) };
    unsafe { f(section, key, default, out, size, file) }
}

pub(crate) fn init() {
    map();
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
                log!("gamewax: {dll}!{name} not imported");
            }
        }
        let files: [(&str, usize, &AtomicUsize); 5] = [
            ("CreateFileA", create_file as *const () as usize, &ORIG_CREATE_FILE),
            ("ReadFile", read_file as *const () as usize, &ORIG_READ_FILE),
            ("WriteFile", write_file as *const () as usize, &ORIG_WRITE_FILE),
            ("CloseHandle", close_handle as *const () as usize, &ORIG_CLOSE_HANDLE),
            ("GetPrivateProfileStringA", get_profile_string as *const () as usize, &ORIG_PROFILE_STRING),
        ];
        for (name, f, orig) in files {
            if let Some(o) = iat::hook("kernel32.dll", name, f) {
                orig.store(o, Ordering::Relaxed);
            }
        }
    }
}
