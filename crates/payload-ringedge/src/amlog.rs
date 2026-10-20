//! `WAL_RINGEDGE_AMLOG=<rva>`: the messages of the game's amLib (Sega's system library, linked
//! in the executable), silent in a release build, written to the payload log. `<rva>` is its
//! log function `(const char *function, int line, const char *format, ...)`, replaced by a
//! stub that formats the message.
//!
//! `WAL_RINGEDGE_GAMELOG=<rva>`: the same for a game's own printf-like log function
//! `(const char *format, ...)` (Transformers: Human Alliance's ArcadeInit messages).
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

// (format, ...): format at [esp + 4], arguments after it.
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "lea eax, [esp + 8]",
    "push eax",
    "push dword ptr [esp + 8]",
    "call {log}",
    "add esp, 8",
    "ret",
    name = sym gamelog_stub,
    log = sym gamelog_va,
);

unsafe extern "C" {
    fn gamelog_stub();
}

core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "pushfd",
    "pushad",
    "mov eax, [esp + 0x34]",
    "push esi",
    "push eax",
    "call {trace}",
    "add esp, 8",
    "popad",
    "popfd",
    "mov eax, [esp + 0x10]",
    "jmp dword ptr [{continuation}]",
    name = sym jvs_state_trace_stub,
    trace = sym jvs_state_trace,
    continuation = sym JVS_INIT_CONTINUATION,
);
static mut JVS_INIT_CONTINUATION: usize = 0;
unsafe extern "C" { fn jvs_state_trace_stub(); }
extern "C" fn jvs_state_trace(status: u32, node: usize) {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    unsafe {
        log!("game: JVS init return status={status:#x} node={node:#x} state={:#x} flags={:08x?}",
            std::ptr::read_unaligned((base + 0x2bdd8c) as *const u32),
            std::slice::from_raw_parts((base + 0x2b0218) as *const u32, 8));
    }
}

extern "C" fn gamelog_va(fmt: *const c_char, args: *const c_void) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0 as c_char; 1024];
    unsafe { _vsnprintf(buf.as_mut_ptr(), buf.len() - 1, fmt, args) };
    let msg = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    log!("game: {}", msg.trim_end());
    if msg.contains("JVS node(s) found") {
        let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let node = base + 0x2b023c;
            unsafe {
                let count = std::ptr::read_unaligned((base + 0x2b0238) as *const u32);
                let bytes = std::slice::from_raw_parts(node as *const u8, 0x300);
                let globals = std::slice::from_raw_parts((base + 0x2b0200) as *const u32, 16);
                let state = std::ptr::read_unaligned((base + 0x2bdd8c) as *const u32);
                log!("game: JVS base={base:#x} count={count} state={state} globals={globals:08x?} node={:02x?}", &bytes[..]);
            }
        });
    }
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
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if let Some(rva) = std::env::var("WAL_RINGEDGE_GAMELOG").ok().and_then(|v| parse_hex(&v)) {
        unsafe { jump_to(base, rva, gamelog_stub as *const () as usize) };
        log!("amlog: game log function at +{rva:#x}");
    }
    if std::env::var_os("WAL_RINGEDGE_JVS_DUMP").is_some() {
        unsafe { JVS_INIT_CONTINUATION = base + 0x8dff4; }
        unsafe { jump_to(base, 0x8dff0, jvs_state_trace_stub as *const () as usize); }
        log!("amlog: JVS init return trace enabled");
    }
    let Some(rva) = std::env::var("WAL_RINGEDGE_AMLOG").ok().and_then(|v| parse_hex(&v)) else { return };
    unsafe { jump_to(base, rva, amlog_stub as *const () as usize) };
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
