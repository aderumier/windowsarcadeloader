//! `WAL_RINGEDGE_AMLOG=<rva>`: the messages of the game's amLib (Sega's system library, linked
//! in the executable), silent in a release build, written to the payload log. `<rva>` is its
//! log function `(const char *function, int line, const char *format, ...)`, replaced by a
//! stub that formats the message.
//!
//! Each amLib module only logs when its own level variable is at least 1:
//! `WAL_RINGEDGE_AMLOG_LEVELS=<rva>[=<level>],...` sets them (default level 1: errors).

use std::ffi::{c_char, c_void};

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

unsafe extern "C" {
    fn _vsnprintf(buf: *mut c_char, size: usize, fmt: *const c_char, args: *const c_void) -> i32;
}

// (function, line, format, ...): each `push [esp + 16]` takes the next argument back to front.
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "lea eax, [esp + 16]",
    "push eax",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call {log}",
    "add esp, 16",
    "ret",
    name = sym amlog_stub,
    log = sym amlog_va,
);

unsafe extern "C" {
    fn amlog_stub();
}

extern "C" fn amlog_va(function: *const c_char, line: u32, fmt: *const c_char, args: *const c_void) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0 as c_char; 1024];
    unsafe { _vsnprintf(buf.as_mut_ptr(), buf.len() - 1, fmt, args) };
    let msg = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    let function = if function.is_null() {
        "?".into()
    } else {
        unsafe { std::ffi::CStr::from_ptr(function) }.to_string_lossy()
    };
    log!("amlib: {function}({line}): {}", msg.trim_end());
}

fn parse_hex(s: &str) -> Option<usize> {
    usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

pub(crate) fn init() {
    let Some(rva) = std::env::var("WAL_RINGEDGE_AMLOG").ok().and_then(|v| parse_hex(&v)) else { return };
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let at = base + rva;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        *(at as *mut u8) = 0xE9;
        let rel = (amlog_stub as *const () as usize).wrapping_sub(at + 5) as i32;
        std::ptr::write_unaligned((at + 1) as *mut i32, rel);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
    let levels = std::env::var("WAL_RINGEDGE_AMLOG_LEVELS").unwrap_or_default();
    let mut count = 0;
    for item in levels.split(',').filter(|s| !s.trim().is_empty()) {
        let (var, level) = item.split_once('=').unwrap_or((item, "1"));
        let (Some(var), Ok(level)) = (parse_hex(var), level.trim().parse::<i32>()) else {
            log!("amlog: ignoring level '{item}'");
            continue;
        };
        unsafe { std::ptr::write_unaligned((base + var) as *mut i32, level) };
        count += 1;
    }
    log!("amlog: amLib log function at +{rva:#x}, {count} level(s) set");
}
