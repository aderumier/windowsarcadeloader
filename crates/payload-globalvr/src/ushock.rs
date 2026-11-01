//! UltraPin's uShock board (`USB\VID_04B4&PID_6470`, a Cypress HID device): the cabinet's
//! knocker and its other outputs.
//!
//! The tables' player (`VPinball.exe`) looks for it in a static initializer (`0x4e8af1`: SetupAPI
//! over the HID class, `CreateFileA` of each interface, `HidD_GetAttributes` vendor 0x04b4
//! product 0x6470) and exits when it is not there; it then writes output reports (overlapped
//! `WriteFile`, byte 1 the output bits: the knocker, `0x4255f9`). The frontend (`plfe.exe`, the W
//! functions) opens it the same way and writes its outputs too.
//!
//! Every call is answered by IAT hooks: one interface, opened as a sentinel handle; the writes
//! succeed (their overlapped event signalled), the reads return a zeroed report.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::SetLastError;

/// The interface path (ANSI and UTF-16 compares).
const DEVICE_PATH: &str = "\\\\?\\hid#vid_04b4&pid_6470#wal#{4d1e55b2-f16f-11cf-88cb-001111000030}";
/// Sentinel handles: the device information set, the opened board.
const DEVINFO: usize = 0x5741_4C00;
const HANDLE: usize = 0x5741_4C10;
/// The preparsed data handed back to `HidP_GetCaps`.
static PREPARSED: u32 = 0x5741_4C50;
/// Report lengths (report id byte included).
const INPUT_LEN: u16 = 8;
const OUTPUT_LEN: u16 = 8;

const HIDP_STATUS_SUCCESS: u32 = 0x0011_0000;
const ERROR_NO_MORE_ITEMS: u32 = 0x103;
const ERROR_INSUFFICIENT_BUFFER: u32 = 0x7a;

/// The last output bits written (logged on change).
static OUTPUTS: AtomicU8 = AtomicU8::new(0);

static ORIG_CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_READ_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_WRITE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLOSE_HANDLE: AtomicUsize = AtomicUsize::new(0);

// ── SetupAPI: one HID interface ───────────────────────────────────────────────────────────────

unsafe extern "system" fn get_class_devs(_guid: *const u8, _enumerator: *const u8, _hwnd: usize, _flags: u32) -> usize {
    DEVINFO
}

unsafe extern "system" fn enum_interfaces(_set: usize, _dev: usize, _guid: *const u8, index: u32, _data: *mut u8) -> i32 {
    if index == 0 {
        return 1;
    }
    unsafe { SetLastError(ERROR_NO_MORE_ITEMS) };
    0
}

/// `SetupDiGetDeviceInterfaceDetailA/W(set, data, detail, size, required, devinfo)`: the size
/// first (no buffer), then `cbSize` + the device path.
unsafe fn interface_detail(detail: *mut u8, size: u32, required: *mut u32, wide: bool) -> i32 {
    let unit = if wide { 2 } else { 1 };
    let needed = 4 + (DEVICE_PATH.len() + 1) * unit;
    unsafe {
        if !required.is_null() {
            *required = needed as u32;
        }
        if detail.is_null() || (size as usize) < needed {
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
            return 0;
        }
        if wide {
            for (i, c) in DEVICE_PATH.encode_utf16().chain(std::iter::once(0)).enumerate() {
                (detail.add(4 + 2 * i) as *mut u16).write_unaligned(c);
            }
        } else {
            std::ptr::copy_nonoverlapping(DEVICE_PATH.as_ptr(), detail.add(4), DEVICE_PATH.len());
            *detail.add(4 + DEVICE_PATH.len()) = 0;
        }
    }
    1
}

unsafe extern "system" fn interface_detail_a(_set: usize, _data: *const u8, detail: *mut u8, size: u32, required: *mut u32, _devinfo: *mut u8) -> i32 {
    unsafe { interface_detail(detail, size, required, false) }
}

unsafe extern "system" fn interface_detail_w(_set: usize, _data: *const u8, detail: *mut u8, size: u32, required: *mut u32, _devinfo: *mut u8) -> i32 {
    unsafe { interface_detail(detail, size, required, true) }
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
        (attr.add(4) as *mut u16).write_unaligned(0x04b4);
        (attr.add(6) as *mut u16).write_unaligned(0x6470);
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

unsafe extern "system" fn free_preparsed_data(_preparsed: usize) -> u8 {
    1
}

/// `HIDP_CAPS` (64 bytes): usage, usage page, input / output / feature report lengths.
unsafe extern "system" fn get_caps(preparsed: usize, caps: *mut u16) -> u32 {
    if caps.is_null() || preparsed != &PREPARSED as *const u32 as usize {
        return 0xc011_0001; // HIDP_STATUS_INVALID_PREPARSED_DATA
    }
    unsafe {
        std::ptr::write_bytes(caps as *mut u8, 0, 64);
        *caps = 1;
        *caps.add(1) = 0xff00;
        *caps.add(2) = INPUT_LEN;
        *caps.add(3) = OUTPUT_LEN;
    }
    HIDP_STATUS_SUCCESS
}

// ── Files: the board ──────────────────────────────────────────────────────────────────────────

type CreateFileA = unsafe extern "system" fn(*const u8, u32, u32, usize, u32, u32, usize) -> usize;
type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, usize, u32, u32, usize) -> usize;
type ReadWriteFile = unsafe extern "system" fn(usize, *mut u8, u32, *mut u32, usize) -> i32;
type CloseHandle = unsafe extern "system" fn(usize) -> i32;

unsafe extern "system" fn create_file_a(name: *const u8, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().eq_ignore_ascii_case(DEVICE_PATH.as_bytes()) {
        log!("ushock: board opened");
        return HANDLE;
    }
    let f: CreateFileA = unsafe { std::mem::transmute(ORIG_CREATE_FILE_A.load(Ordering::Relaxed)) };
    unsafe { f(name, access, share, sa, disp, flags, template) }
}

unsafe extern "system" fn create_file_w(name: *const u16, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        let n = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) });
        if n.eq_ignore_ascii_case(DEVICE_PATH) {
            log!("ushock: board opened");
            return HANDLE;
        }
    }
    let f: CreateFileW = unsafe { std::mem::transmute(ORIG_CREATE_FILE_W.load(Ordering::Relaxed)) };
    unsafe { f(name, access, share, sa, disp, flags, template) }
}

/// An overlapped transfer done: its event (`OVERLAPPED.hEvent`, offset 16) signalled.
unsafe fn complete(overlapped: usize, done: *mut u32, len: u32) {
    unsafe {
        if !done.is_null() {
            *done = len;
        }
        if overlapped != 0 {
            *(overlapped as *mut usize) = 0; // Internal: STATUS_SUCCESS
            *((overlapped + 4) as *mut usize) = len as usize; // InternalHigh: bytes transferred
            let event = *((overlapped + 16) as *const usize);
            if event != 0 {
                windows_sys::Win32::System::Threading::SetEvent(event as _);
            }
        }
    }
}

unsafe extern "system" fn read_file(handle: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32 {
    if handle != HANDLE {
        let f: ReadWriteFile = unsafe { std::mem::transmute(ORIG_READ_FILE.load(Ordering::Relaxed)) };
        return unsafe { f(handle, buf, len, read, overlapped) };
    }
    let n = len.min(INPUT_LEN as u32);
    unsafe {
        std::ptr::write_bytes(buf, 0, n as usize);
        complete(overlapped, read, n);
    }
    1
}

unsafe extern "system" fn write_file(handle: usize, buf: *mut u8, len: u32, written: *mut u32, overlapped: usize) -> i32 {
    if handle != HANDLE {
        let f: ReadWriteFile = unsafe { std::mem::transmute(ORIG_WRITE_FILE.load(Ordering::Relaxed)) };
        return unsafe { f(handle, buf, len, written, overlapped) };
    }
    if len >= 2 && !buf.is_null() {
        let bits = unsafe { *buf.add(1) };
        if OUTPUTS.swap(bits, Ordering::Relaxed) != bits {
            log!("ushock: outputs {bits:#04x}");
        }
    }
    unsafe { complete(overlapped, written, len) };
    1
}

unsafe extern "system" fn close_handle(handle: usize) -> i32 {
    if handle == HANDLE {
        return 1;
    }
    let f: CloseHandle = unsafe { std::mem::transmute(ORIG_CLOSE_HANDLE.load(Ordering::Relaxed)) };
    unsafe { f(handle) }
}

/// Hooks the executable's imports (the frontend's and the tables' player's).
pub(crate) fn init() {
    unsafe {
        let hooks: [(&str, &str, usize); 10] = [
            ("setupapi.dll", "SetupDiGetClassDevsA", get_class_devs as *const () as usize),
            ("setupapi.dll", "SetupDiGetClassDevsW", get_class_devs as *const () as usize),
            ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize),
            ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA", interface_detail_a as *const () as usize),
            ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailW", interface_detail_w as *const () as usize),
            ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy_info_list as *const () as usize),
            ("hid.dll", "HidD_GetAttributes", get_attributes as *const () as usize),
            ("hid.dll", "HidD_GetPreparsedData", get_preparsed_data as *const () as usize),
            ("hid.dll", "HidD_FreePreparsedData", free_preparsed_data as *const () as usize),
            ("hid.dll", "HidP_GetCaps", get_caps as *const () as usize),
        ];
        for (dll, name, f) in hooks {
            let _ = iat::hook(dll, name, f);
        }
        let files: [(&str, usize, &AtomicUsize); 5] = [
            ("CreateFileA", create_file_a as *const () as usize, &ORIG_CREATE_FILE_A),
            ("CreateFileW", create_file_w as *const () as usize, &ORIG_CREATE_FILE_W),
            ("ReadFile", read_file as *const () as usize, &ORIG_READ_FILE),
            ("WriteFile", write_file as *const () as usize, &ORIG_WRITE_FILE),
            ("CloseHandle", close_handle as *const () as usize, &ORIG_CLOSE_HANDLE),
        ];
        for (name, f, orig) in files {
            if let Some(o) = iat::hook("kernel32.dll", name, f) {
                orig.store(o, Ordering::Relaxed);
            }
        }
    }
    log!("ushock: board emulated");
}
