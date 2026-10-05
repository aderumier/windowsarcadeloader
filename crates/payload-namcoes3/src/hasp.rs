//! HASP HL API (`hasp_windows_x64_100610.dll`): every call succeeds, `hasp_read` returns the
//! dongle memory of Star Wars Battle Pod (game id, cabinet serial at 0xd00).
//!
//! Outside Japanese, the game checks the HASP serial: its first 6 characters must match the
//! game's run-time serial template (0x1573d28), so in the game process the serial is built
//! from it.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

use wal_payload_common::log;

const STATUS_OK: i32 = 0;
const SIZE: usize = 0xD40;

fn memory() -> &'static [u8; SIZE] {
    static MEMORY: OnceLock<[u8; SIZE]> = OnceLock::new();
    MEMORY.get_or_init(|| {
        let mut m = [0u8; SIZE];
        for (at, b) in [
            (0x00, 0x01),
            (0x13, 0x01),
            (0x17, 0x0A),
            (0x1B, 0x04),
            (0x1C, 0x3B),
            (0x1D, 0x6B),
            (0x1E, 0x40),
            (0x1F, 0x87),
            (0x23, 0x01),
            (0x27, 0x0A),
            (0x2B, 0x04),
            (0x2C, 0x3B),
            (0x2D, 0x6B),
            (0x2E, 0x40),
            (0x2F, 0x87),
            (0xD3E, 0x6A),
            (0xD3F, 0x95),
        ] {
            m[at] = b;
        }
        m[0xD00..0xD0C].copy_from_slice(b"274320990002");
        m
    })
}

/// `hasp_get_info` / `hasp_get_sessioninfo` answer.
static INFO: &[u8] =
    b"<?xml version=\"1.0\" encoding=\"UTF-8\" ?><hasp_info><hasp><haspid>274320990</haspid></hasp></hasp_info>\0";

unsafe fn set<T>(p: *mut T, v: T) {
    if !p.is_null() {
        unsafe { *p = v };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_login(feature: u32, _vendor: *const c_void, handle: *mut u32) -> i32 {
    log!("hasp: login feature {feature:#x}");
    unsafe { set(handle, 1) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_login_scope(feature: u32, _scope: *const c_char, _vendor: *const c_void, handle: *mut u32) -> i32 {
    log!("hasp: login_scope feature {feature:#x}");
    unsafe { set(handle, 1) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_login_port(_f: u32, _port: *const c_char, _vendor: *const c_void, handle: *mut u32) -> i32 {
    unsafe { set(handle, 1) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_get_size(_h: u32, _file: u32, size: *mut u32) -> i32 {
    unsafe { set(size, SIZE as u32) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_read(_h: u32, file: u32, offset: u32, len: u32, buf: *mut u8) -> i32 {
    log!("hasp: read file {file:#x} offset {offset:#x} length {len:#x}");
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    out.fill(0);
    let mut mem = *memory();
    if crate::game_base() != 0 {
        mem[0xD00..0xD0C].copy_from_slice(&crate::patches::serial(0x1573d28));
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| log!("hasp: serial {}", String::from_utf8_lossy(&mem[0xD00..0xD0C])));
    }
    let start = (offset as usize).min(SIZE);
    let n = (SIZE - start).min(out.len());
    out[..n].copy_from_slice(&mem[start..start + n]);
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_write(_h: u32, file: u32, offset: u32, len: u32, _buf: *const u8) -> i32 {
    log!("hasp: write file {file:#x} offset {offset:#x} length {len:#x} (ignored)");
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_get_sessioninfo(_h: u32, _format: *const c_char, info: *mut *const u8) -> i32 {
    unsafe { set(info, INFO.as_ptr()) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_get_info(_scope: *const c_char, _format: *const c_char, _vendor: *const c_void, info: *mut *const u8) -> i32 {
    unsafe { set(info, INFO.as_ptr()) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_get_version(major: *mut u32, minor: *mut u32, server: *mut u32, number: *mut u32, _vendor: *const c_void) -> i32 {
    unsafe {
        set(major, 4);
        set(minor, 0);
        set(server, 0);
        set(number, 0);
    }
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_get_rtc(_h: u32, time: *mut u64) -> i32 {
    unsafe { set(time, 0) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_datetime_to_hasptime(_d: u32, _mo: u32, _y: u32, _h: u32, _mi: u32, _s: u32, time: *mut u64) -> i32 {
    unsafe { set(time, 0) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_update(_update: *const c_char, ack: *mut *const u8) -> i32 {
    unsafe { set(ack, std::ptr::null()) };
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_get_trace(trace: *mut *const u8) -> i32 {
    unsafe { set(trace, std::ptr::null()) };
    STATUS_OK
}

/// Calls that only need to succeed.
macro_rules! ok {
    ($($name:ident($($a:ident: $t:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($a: $t),*) -> i32 {
            STATUS_OK
        }
    )*};
}

ok! {
    hasp_logout(_h: u32);
    hasp_encrypt(_h: u32, _buf: *mut c_void, _len: u32);
    hasp_decrypt(_h: u32, _buf: *mut c_void, _len: u32);
    hasp_legacy_encrypt(_h: u32, _buf: *mut c_void, _len: u32);
    hasp_legacy_decrypt(_h: u32, _buf: *mut c_void, _len: u32);
    hasp_legacy_set_idletime(_h: u32, _t: u16);
    hasp_legacy_set_rtc(_h: u32, _t: u64);
    hasp_hasptime_to_datetime(_t: u64, _d: *mut u32, _mo: *mut u32, _y: *mut u32, _h: *mut u32, _mi: *mut u32, _s: *mut u32);
    hasp_enable_trace(_level: u32, _path: *const c_char);
    hasp_login_ex();
    hasp_detach();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hasp_free(_info: *mut c_char) {}
