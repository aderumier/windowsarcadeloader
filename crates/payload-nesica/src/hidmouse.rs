//! A HID mouse for SetupAPI.
//!
//! Left 4 Dead: Survivors' `inputsystem.dll` looks for its gun (a USB mouse) with
//! `SetupDiGetClassDevsA(NULL, "HID", ...)` and wants a device whose class (`SPDRP_CLASS`) is
//! `Mouse`, else "I/O error 4". Wine lists no mouse among the HID devices: the module's
//! enumeration of `HID` is answered with one such device.
//!
//! Option: `WAL_NESICA_HID_MOUSE=<module>` (the module enumerating, `inputsystem.dll`).

use std::ffi::{CStr, c_char};
use std::sync::OnceLock;

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, SetLastError};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

/// The fake device information set.
const SET: usize = 0x4D4F_5553;
const SPDRP_CLASS: u32 = 7;
const CLASS: &[u8] = b"Mouse\0";

struct Real {
    class_devs: usize,
    enum_info: usize,
    property: usize,
    destroy: usize,
}

static REAL: OnceLock<Real> = OnceLock::new();

extern "system" fn class_devs(guid: usize, enumerator: *const c_char, hwnd: usize, flags: u32) -> usize {
    if !enumerator.is_null() && unsafe { CStr::from_ptr(enumerator) }.to_bytes().eq_ignore_ascii_case(b"HID") {
        log!("hidmouse: HID devices listed: one mouse");
        return SET;
    }
    let f: extern "system" fn(usize, *const c_char, usize, u32) -> usize = unsafe { std::mem::transmute(REAL.get().unwrap().class_devs) };
    f(guid, enumerator, hwnd, flags)
}

extern "system" fn enum_info(set: usize, index: u32, data: *mut u8) -> i32 {
    if set == SET {
        if index > 0 {
            unsafe { SetLastError(ERROR_NO_MORE_ITEMS) };
            return 0;
        }
        // SP_DEVINFO_DATA: cbSize, ClassGuid, DevInst, Reserved (left zero)
        if !data.is_null() {
            unsafe { std::ptr::write_bytes(data.add(4), 0, 24) };
        }
        return 1;
    }
    let f: extern "system" fn(usize, u32, *mut u8) -> i32 = unsafe { std::mem::transmute(REAL.get().unwrap().enum_info) };
    f(set, index, data)
}

extern "system" fn property(set: usize, data: *mut u8, prop: u32, kind: *mut u32, buf: *mut u8, size: u32, required: *mut u32) -> i32 {
    if set == SET {
        if prop != SPDRP_CLASS {
            unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_INVALID_DATA) };
            return 0;
        }
        if !kind.is_null() {
            unsafe { *kind = 1 }; // REG_SZ
        }
        if !required.is_null() {
            unsafe { *required = CLASS.len() as u32 };
        }
        if buf.is_null() || (size as usize) < CLASS.len() {
            unsafe { SetLastError(windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER) };
            return 0;
        }
        unsafe { std::ptr::copy_nonoverlapping(CLASS.as_ptr(), buf, CLASS.len()) };
        return 1;
    }
    let f: extern "system" fn(usize, *mut u8, u32, *mut u32, *mut u8, u32, *mut u32) -> i32 = unsafe { std::mem::transmute(REAL.get().unwrap().property) };
    f(set, data, prop, kind, buf, size, required)
}

extern "system" fn destroy(set: usize) -> i32 {
    if set == SET {
        return 1;
    }
    let f: extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(REAL.get().unwrap().destroy) };
    f(set)
}

pub(crate) fn init() {
    let Ok(name) = std::env::var("WAL_NESICA_HID_MOUSE") else { return };
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let base = unsafe { GetModuleHandleW(wide.as_ptr()) } as usize;
    if base == 0 {
        log!("hidmouse: {name} not loaded");
        return;
    }
    let hook = |function: &str, f: usize| unsafe { iat::hook_module(base, "setupapi.dll", function, f) }.unwrap_or(0);
    let real = Real {
        class_devs: hook("SetupDiGetClassDevsA", class_devs as *const () as usize),
        enum_info: hook("SetupDiEnumDeviceInfo", enum_info as *const () as usize),
        property: hook("SetupDiGetDeviceRegistryPropertyA", property as *const () as usize),
        destroy: hook("SetupDiDestroyDeviceInfoList", destroy as *const () as usize),
    };
    if real.class_devs == 0 || real.enum_info == 0 || real.property == 0 || real.destroy == 0 {
        log!("hidmouse: {name} does not import the SetupAPI functions");
    }
    let _ = REAL.set(real);
}
