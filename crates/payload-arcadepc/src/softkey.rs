//! The "SoftKey" USB HID dongle of JX's camera-gun cabinet (Top Gun 2: `GameMain.CheckKey`).
//!
//! The game's `Encrypt.SoftKey` class finds the dongle with `SetupDiGetClassDevsA`,
//! `SetupDiEnumDeviceInterfaces`, `SetupDiGetDeviceInterfaceDetailA` (ANSI path), `CreateFile`
//! and `HidD_GetAttributes` (vendor 0x3689, product 0x8762), then reads its EEPROM with feature
//! reports: `HidD_SetFeature` (report 2: command 0x12, the address's high byte doubled, its
//! low byte, the length, an 8-byte password), `HidD_GetFeature` (report 1: status 0, the
//! bytes). `CheckKeyByReadEprom`: the length at address 0, then the string after it, which
//! must be `JXKJ88188188123`. All its `DllImport`s resolved to the payload's
//! (`payload-common`'s `mono_imports.rs`): one device, its handles the payload's, every other
//! handle the real functions' (or the serial ports' replacement).

use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;

use wal_payload_common::{log, serial};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

const VENDOR: u16 = 0x3689;
const PRODUCT: u16 = 0x8762;
/// The EEPROM's string (its length byte at address 0).
const EEPROM: &[u8] = b"JXKJ88188188123";
/// The feature reports' length (`HidP_GetCaps`), report id included.
const FEATURE_LEN: u16 = 65;

const PATH: &str = r"\\?\hid#vid_3689&pid_8762#wal-softkey#{4d1e55b2-f16f-11cf-88cb-001111000030}";
/// The device list's handle, the device's file handles and its preparsed data: far from
/// Wine's (small multiples of 4).
const DEVICE_LIST: usize = 0x5A31_0000;
const HANDLE_BASE: usize = 0x5A32_0000;
const PREPARSED: usize = 0x5A33_0000;

static HANDLES: AtomicUsize = AtomicUsize::new(0);
/// The last read asked (address << 8 | length), answered by the next `HidD_GetFeature`.
static READ: AtomicU32 = AtomicU32::new(0);

fn ours(handle: usize) -> bool {
    (HANDLE_BASE..HANDLE_BASE + 0x1000).contains(&handle)
}

/// The EEPROM's byte at `address` (0xFF where nothing is written).
fn eeprom(address: usize) -> u8 {
    match address {
        0 => EEPROM.len() as u8,
        a if a <= EEPROM.len() => EEPROM[a - 1],
        _ => 0xFF,
    }
}

/// A real function, for the handles not the device's.
fn real(module: &str, name: &str) -> usize {
    let module: Vec<u16> = module.encode_utf16().chain([0]).collect();
    let name = format!("{name}\0");
    let m = unsafe { GetModuleHandleW(module.as_ptr()) };
    unsafe { GetProcAddress(m, name.as_ptr()) }.map_or(0, |f| f as usize)
}

/// The serial ports' replacement of a kernel32 function (it chains to the real one), else
/// the real one.
fn next(name: &'static str) -> usize {
    serial::replacement(name).unwrap_or_else(|| real("kernel32.dll", name))
}

macro_rules! next {
    ($name:literal, $t:ty) => {{
        static F: OnceLock<usize> = OnceLock::new();
        let f = *F.get_or_init(|| next($name));
        unsafe { std::mem::transmute::<usize, $t>(f) }
    }};
}

// setupapi: one HID interface, the dongle's

extern "system" fn get_class_devs(_guid: *const u8, _enumerator: usize, _hwnd: usize, _flags: u32) -> usize {
    DEVICE_LIST
}

/// `SetupDiEnumDeviceInterfaces(set, devinfo, guid, index, data)`: `data` is `cbSize`, the
/// interface class's GUID, flags, reserved. The game reads `GetLastError` after a success
/// (259: the end): cleared.
extern "system" fn enum_interfaces(_set: usize, _dev: usize, guid: *const u8, index: i32, data: *mut u8) -> i32 {
    use windows_sys::Win32::Foundation::SetLastError;
    if index != 0 || data.is_null() {
        unsafe { SetLastError(259) };
        return 0;
    }
    unsafe {
        if !guid.is_null() {
            std::ptr::copy_nonoverlapping(guid, data.add(4), 16);
        }
        *(data.add(20) as *mut u32) = 1;
        SetLastError(0);
    }
    1
}

/// `SetupDiGetDeviceInterfaceDetailA(set, data, detail, size, required, devinfo)`: the path
/// after `cbSize`.
extern "system" fn interface_detail(_set: usize, _data: *const u8, detail: *mut u8, size: u32, required: *mut u32, _devinfo: *mut u8) -> i32 {
    let need = 4 + PATH.len() as u32 + 1;
    if !required.is_null() {
        unsafe { *required = need };
    }
    if detail.is_null() || size < need {
        unsafe { windows_sys::Win32::Foundation::SetLastError(122) };
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(PATH.as_ptr(), detail.add(4), PATH.len());
        *detail.add(4 + PATH.len()) = 0;
    }
    1
}

extern "system" fn destroy_info_list(_set: usize) -> i32 {
    1
}

// hid: the dongle's attributes and feature reports

/// `HIDD_ATTRIBUTES`: size, vendor, product, version.
extern "system" fn get_attributes(handle: usize, attr: *mut u16) -> u8 {
    if !ours(handle) {
        let f: unsafe extern "system" fn(usize, *mut u16) -> u8 = unsafe { std::mem::transmute(real("hid.dll", "HidD_GetAttributes")) };
        return unsafe { f(handle, attr) };
    }
    unsafe {
        *(attr as *mut u32) = 12;
        *attr.add(2) = VENDOR;
        *attr.add(3) = PRODUCT;
        *attr.add(4) = 0x0100;
    }
    1
}

extern "system" fn get_preparsed_data(handle: usize, data: *mut usize) -> u8 {
    if !ours(handle) {
        let f: unsafe extern "system" fn(usize, *mut usize) -> u8 = unsafe { std::mem::transmute(real("hid.dll", "HidD_GetPreparsedData")) };
        return unsafe { f(handle, data) };
    }
    unsafe { *data = PREPARSED };
    1
}

extern "system" fn free_preparsed_data(data: usize) -> u8 {
    if data != PREPARSED {
        let f: unsafe extern "system" fn(usize) -> u8 = unsafe { std::mem::transmute(real("hid.dll", "HidD_FreePreparsedData")) };
        return unsafe { f(data) };
    }
    1
}

/// `HidP_GetCaps`: `HIDP_CAPS` (usage, usage page, the input, output and feature reports'
/// lengths...); `HIDP_STATUS_SUCCESS`.
extern "system" fn get_caps(data: usize, caps: *mut u16) -> i32 {
    if data != PREPARSED {
        let f: unsafe extern "system" fn(usize, *mut u16) -> i32 = unsafe { std::mem::transmute(real("hid.dll", "HidP_GetCaps")) };
        return unsafe { f(data, caps) };
    }
    unsafe {
        std::ptr::write_bytes(caps, 0, 32);
        *caps = 1;
        *caps.add(1) = 0xFF00;
        *caps.add(4) = FEATURE_LEN;
    }
    0x0011_0000
}

/// `HidD_SetFeature`: report 2, the command at 1 (0x12: an EEPROM read).
extern "system" fn set_feature(handle: usize, buf: *const u8, len: u32) -> u8 {
    if !ours(handle) {
        let f: unsafe extern "system" fn(usize, *const u8, u32) -> u8 = unsafe { std::mem::transmute(real("hid.dll", "HidD_SetFeature")) };
        return unsafe { f(handle, buf, len) };
    }
    if buf.is_null() || len < 5 {
        return 0;
    }
    let report = unsafe { std::slice::from_raw_parts(buf, 5) };
    match report[1] {
        0x12 => {
            let address = (report[2] as u32 / 2) << 8 | report[3] as u32;
            READ.store(address << 8 | report[4] as u32, Ordering::Relaxed);
        }
        cmd => log!("softkey: command {cmd:#04x} not emulated"),
    }
    1
}

/// `HidD_GetFeature`: report 1, status 0 and the bytes read.
extern "system" fn get_feature(handle: usize, buf: *mut u8, len: u32) -> u8 {
    if !ours(handle) {
        let f: unsafe extern "system" fn(usize, *mut u8, u32) -> u8 = unsafe { std::mem::transmute(real("hid.dll", "HidD_GetFeature")) };
        return unsafe { f(handle, buf, len) };
    }
    if buf.is_null() || len == 0 {
        return 0;
    }
    let read = READ.load(Ordering::Relaxed);
    let (address, n) = ((read >> 8) as usize, (read & 0xFF) as usize);
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    out.fill(0);
    for (i, b) in out[1..].iter_mut().take(n).enumerate() {
        *b = eeprom(address + i);
    }
    1
}

// kernel32: the dongle's handles

extern "system" fn create_file(name: *const c_char, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    let path = if name.is_null() { "" } else { unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("") };
    if !path.contains("wal-softkey") {
        let f = next!("CreateFileA", unsafe extern "system" fn(*const c_char, u32, u32, usize, u32, u32, usize) -> usize);
        return unsafe { f(name, access, share, sa, disp, flags, template) };
    }
    if HANDLES.load(Ordering::Relaxed) == 0 {
        log!("softkey: the dongle opened");
    }
    HANDLE_BASE + 4 * (HANDLES.fetch_add(1, Ordering::Relaxed) % 0x400)
}

extern "system" fn close_handle(handle: usize) -> i32 {
    if ours(handle) {
        return 1;
    }
    let f = next!("CloseHandle", unsafe extern "system" fn(usize) -> i32);
    unsafe { f(handle) }
}

/// The dongle's functions (a `mono_imports` resolver's part). `CreateFile` is answered by its
/// ANSI name only: Mono's `SerialPort` (Unicode) asks `CreateFile` too.
pub fn resolve(name: &str, module: &str) -> Option<usize> {
    let f = |f: usize| Some(f);
    match (module, name) {
        ("setupapi.dll", "SetupDiGetClassDevsA") => f(get_class_devs as *const () as usize),
        ("setupapi.dll", "SetupDiEnumDeviceInterfaces") => f(enum_interfaces as *const () as usize),
        ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA") => f(interface_detail as *const () as usize),
        ("setupapi.dll", "SetupDiDestroyDeviceInfoList") => f(destroy_info_list as *const () as usize),
        ("hid.dll", "HidD_GetAttributes") => f(get_attributes as *const () as usize),
        ("hid.dll", "HidD_GetPreparsedData") => f(get_preparsed_data as *const () as usize),
        ("hid.dll", "HidD_FreePreparsedData") => f(free_preparsed_data as *const () as usize),
        ("hid.dll", "HidP_GetCaps") => f(get_caps as *const () as usize),
        ("hid.dll", "HidD_SetFeature") => f(set_feature as *const () as usize),
        ("hid.dll", "HidD_GetFeature") => f(get_feature as *const () as usize),
        ("kernel32.dll", "CreateFileA") => f(create_file as *const () as usize),
        ("kernel32.dll", "CloseHandle") => f(close_handle as *const () as usize),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eeprom_string() {
        let s: Vec<u8> = (1..=eeprom(0) as usize).map(eeprom).collect();
        assert_eq!(s, EEPROM);
        assert_eq!(eeprom(EEPROM.len() + 1), 0xFF);
    }
}
