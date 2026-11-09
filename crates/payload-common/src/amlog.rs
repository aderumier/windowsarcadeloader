//! `WAL_<SYSTEM>_AMLOG=<rva>` (`WAL_SEGANU_AMLOG`, `WAL_ALLS_AMLOG`): the messages of the game's amLib (Sega's system library, linked
//! in the executable), silent in a release build, written to the payload log. `<rva>` is its
//! log function `(const char *function, int line, const char *format, ...)`, replaced by a
//! stub that formats the message (found by the calls after `push <function name>`: amLib's
//! error strings name every function).
//!
//! Each amLib module only logs when its own level variable is at least 1 (`cmp [level], 1;
//! jl` before the call): `WAL_<SYSTEM>_AMLOG_LEVELS=<rva>[=<level>],...` sets them (default
//! level 1: errors; spaces also separate them, for `WAL_EXE_ENV`).
//!
//! `WAL_<SYSTEM>_GAMELOG=<rva>`: the same for the game's own printf-like log function
//! `(const char *format, ...)` (Sonic Dash Extreme: its managers' init and state messages).

use std::ffi::{c_char, c_void};

use crate::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

unsafe extern "C" {
    fn _vsnprintf(buf: *mut c_char, size: usize, fmt: *const c_char, args: *const c_void) -> i32;
}

// (function, line, format, ...): each `push [esp + 16]` takes the next argument back to front.
#[cfg(target_arch = "x86")]
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
    name = sym common_amlog_stub,
    log = sym amlog_va,
);

unsafe extern "C" {
    fn common_amlog_stub();
}

// 64-bit: the first variable argument (r9) stored in its home slot, the others follow it on
// the stack: the list starts there.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "mov [rsp + 32], r9",
    "sub rsp, 40",
    "lea r9, [rsp + 72]",
    "call {log}",
    "add rsp, 40",
    "ret",
    name = sym common_amlog_stub,
    log = sym amlog_va,
);

// (format, ...): format at [esp + 4], arguments after it.
#[cfg(target_arch = "x86")]
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "lea eax, [esp + 8]",
    "push eax",
    "push dword ptr [esp + 8]",
    "call {log}",
    "add esp, 8",
    "ret",
    name = sym common_gamelog_stub,
    log = sym gamelog_va,
);

// 64-bit: the variable arguments in rdx, r8, r9 stored in their home slots.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "mov [rsp + 16], rdx",
    "mov [rsp + 24], r8",
    "mov [rsp + 32], r9",
    "sub rsp, 40",
    "lea rdx, [rsp + 56]",
    "call {log}",
    "add rsp, 40",
    "ret",
    name = sym common_gamelog_stub,
    log = sym gamelog_va,
);

unsafe extern "C" {
    fn common_gamelog_stub();
}

extern "C" fn gamelog_va(fmt: *const c_char, args: *const c_void) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0 as c_char; 1024];
    unsafe { _vsnprintf(buf.as_mut_ptr(), buf.len() - 1, fmt, args) };
    let msg = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    log!("game: {}", msg.trim_end());
}

/// Replaces the function at `base + rva` by a jump to `stub`.
unsafe fn jump_to(base: usize, rva: usize, stub: usize) {
    let at = base + rva;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        *(at as *mut u8) = 0xE9;
        std::ptr::write_unaligned((at + 1) as *mut i32, stub.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
}

extern "C" fn amlog_va(function: *const c_char, line: u32, fmt: *const c_char, args: *const c_void) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0 as c_char; 1024];
    unsafe { _vsnprintf(buf.as_mut_ptr(), buf.len() - 1, fmt, args) };
    let msg = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    let function = if function.is_null() { "?".into() } else { unsafe { std::ffi::CStr::from_ptr(function) }.to_string_lossy() };
    log!("amlib: {function}({line}): {}", msg.trim_end());
}

fn parse_hex(s: &str) -> Option<usize> {
    usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

/// The options `WAL_<system>_AMLOG`, `_AMLOG_LEVELS`, `_GAMELOG` (`system`: `SEGANU`, `ALLS`).
pub fn init(system: &str) {
    let var = |name: &str| std::env::var(format!("WAL_{system}_{name}")).ok();
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if let Some(rva) = var("GAMELOG").and_then(|v| parse_hex(&v)) {
        unsafe { jump_to(base, rva, common_gamelog_stub as *const () as usize) };
        log!("amlog: game log function at +{rva:#x}");
    }
    let Some(rva) = var("AMLOG").and_then(|v| parse_hex(&v)) else { return };
    unsafe { jump_to(base, rva, common_amlog_stub as *const () as usize) };
    let levels = var("AMLOG_LEVELS").unwrap_or_default();
    let mut count = 0;
    // commas or spaces (spaces inside `WAL_EXE_ENV`, whose list takes the commas)
    for item in levels.split([',', ' ']).filter(|s| !s.trim().is_empty()) {
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
