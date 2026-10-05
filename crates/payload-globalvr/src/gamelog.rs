//! `WAL_GLOBALVR_GAMELOG=1`: the game's own messages, silent in the release build, written to
//! the payload log. Each printf-like function is entered through a stub that formats the
//! message, then continues into the function (its first instructions are copied to a
//! trampoline).
//!
//! * `0x8773d0` `(lua_State *, fmt, ...)`: script binding errors, raised as Lua errors (the
//!   calling script function is aborted).
//! * `0x885bf0` `(lua_State *, fmt, ...)`: Lua runtime errors (`attempt to call a nil value`...).
//! * `0x406000` `(fmt, ...)`, `0x585240` `(fmt, ...)`: engine messages.
//! * `0x4031c0` `(a, b, c, d, fmt, ...)`: engine warnings.

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect,
};

unsafe extern "C" {
    fn _vsnprintf(buf: *mut c_char, size: usize, fmt: *const c_char, args: *const c_void) -> i32;
}

/// Where each function continues after the message is logged.
static TRAMP: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

// stub_N: fmt at [esp + FMT], arguments right after it; eax/ecx/edx are free at function entry.
macro_rules! stub {
    ($name:ident, $fmt:literal, $idx:literal) => {
        core::arch::global_asm!(
            concat!(".globl {name}\n{name}:"),
            concat!("lea eax, [esp + ", $fmt, " + 4]"),
            "push eax",
            concat!("push dword ptr [esp + ", $fmt, " + 4]"),
            "call {log}",
            "add esp, 8",
            concat!("jmp dword ptr [{tramp} + ", $idx, " * 4]"),
            name = sym $name,
            log = sym log_va,
            tramp = sym TRAMP,
        );
        unsafe extern "C" {
            fn $name();
        }
    };
}

stub!(stub_script_error, 8, 0);
stub!(stub_engine, 4, 1);
stub!(stub_engine2, 4, 2);
stub!(stub_warning, 0x14, 3);
stub!(stub_lua_error, 8, 4);

extern "C" fn log_va(fmt: *const c_char, args: *const c_void) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0 as c_char; 1024];
    unsafe { _vsnprintf(buf.as_mut_ptr(), buf.len() - 1, fmt, args) };
    let msg = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    log!("game: {}", msg.trim_end());
}

/// (image offset, bytes copied to the trampoline: whole instructions, no relative ones)
const HOOKS: [(usize, usize); 5] = [(0x4773d0, 10), (0x6000, 5), (0x185240, 5), (0x31c0, 5), (0x485bf0, 10)];

pub(crate) fn init() {
    if std::env::var("WAL_GLOBALVR_GAMELOG").map_or(true, |v| v != "1") {
        return;
    }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let stubs: [unsafe extern "C" fn(); 5] =
        [stub_script_error, stub_engine, stub_engine2, stub_warning, stub_lua_error];
    let mem = unsafe { VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) }
        as *mut u8;
    for (i, ((rva, len), stub)) in HOOKS.iter().zip(stubs).enumerate() {
        let at = base + rva;
        let tramp = unsafe { mem.add(i * 32) };
        unsafe {
            // copied instructions, then jmp back after them
            std::ptr::copy_nonoverlapping(at as *const u8, tramp, *len);
            *tramp.add(*len) = 0xE9;
            let back = (at + len).wrapping_sub(tramp as usize + len + 5) as i32;
            std::ptr::write_unaligned(tramp.add(len + 1) as *mut i32, back);
        }
        TRAMP[i].store(tramp as usize, Ordering::Relaxed);
        let mut old = 0;
        unsafe {
            VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
            *(at as *mut u8) = 0xE9;
            let rel = (stub as usize).wrapping_sub(at + 5) as i32;
            std::ptr::write_unaligned((at + 1) as *mut i32, rel);
            VirtualProtect(at as *const c_void, 5, old, &mut old);
        }
    }
    log!("gamelog: {} game log functions hooked", HOOKS.len());
}
