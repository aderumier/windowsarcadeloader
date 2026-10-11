//! The Swarm: imports the dump left unresolved.
//!
//! `the_swarm_controls.exe` is a dump of the game unpacked from its HASP envelope, which had
//! redirected some calls of CRT functions: a `call [import]` (6 bytes) became `call thunk` plus
//! a junk byte, each thunk `call 0xbd9160; dd <import id>` into the envelope's resolver. The
//! dump has no resolver (zeros at `0xbd9160`): such a call crashes the game ("Stack overflow"
//! under Windows, an access violation writing 0x1 under Wine). Lua's error paths go through
//! some (`iscntrl` of the syntax error messages, `strncat` of the error positions): a Lua
//! error crashed the game, e.g. at the stage selection after a stage.
//!
//! The resolver is replaced: an import id below (found from its call sites) goes to its
//! msvcr90.dll function, and its call site is rewritten `call function; nop`; another id is
//! logged and the call returns 0.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::Mutex;

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The envelope's resolver, zeros in the dump.
const RESOLVER: usize = 0x7d9160;
/// A thunk checked before patching: `call RESOLVER; dd 0x1643a441` (iscntrl).
const THUNK: usize = 0x3cde1b;

/// Import ids, from their call sites.
const IMPORTS: [(u32, &[u8]); 6] = [
    // luaX_token2str: `iscntrl(token)` of a syntax error message
    (0x1643a441, b"iscntrl\0"),
    // luaO_chunkid: `strncat(out, source, len)` of an error position
    (0x1643d444, b"strncat\0"),
    // l_strcmp: `strcoll(l, r)` of a string comparison
    (0x1641e16f, b"strcoll\0"),
    // db_debug: `fputs("lua_debug> ", stderr)`, `fgets(buffer, 250, stdin)`, `stderr`
    (0x1642850c, b"fputs\0"),
    (0x16428b7c, b"fgets\0"),
    (0x16429fd7, b"__iob_func\0"),
];

// Entered by the thunk's call: [esp] = its import id, [esp + 4] = the return address into the
// call site, before the junk byte. ecx/edx are kept (arguments of a thiscall/fastcall import).
core::arch::global_asm!(
    ".globl {stub}",
    "{stub}:",
    "pop eax",
    "inc dword ptr [esp]",
    "push ecx",
    "push edx",
    "push dword ptr [esp + 8]",
    "push dword ptr [eax]",
    "call {resolve}",
    "add esp, 8",
    "pop edx",
    "pop ecx",
    "test eax, eax",
    "jz .Lswarm_import_unknown",
    "jmp eax",
    ".Lswarm_import_unknown:",
    "ret",
    stub = sym import_stub,
    resolve = sym resolve,
);

unsafe extern "C" {
    fn import_stub();
}

static UNKNOWN: Mutex<Option<HashSet<(u32, usize)>>> = Mutex::new(None);

/// The function of import `id` called from the call site ending at `ret` (past its junk
/// byte), or 0.
extern "C" fn resolve(id: u32, ret: usize) -> usize {
    let site = ret - 6;
    let function = IMPORTS.iter().find(|(i, _)| *i == id).map_or(0, |(_, name)| unsafe {
        let crt = GetModuleHandleA(b"msvcr90.dll\0".as_ptr());
        if crt.is_null() { 0 } else { GetProcAddress(crt, name.as_ptr()).map_or(0, |f| f as usize) }
    });
    if function == 0 {
        if UNKNOWN.lock().unwrap().get_or_insert_default().insert((id, site)) {
            log!("swarm: unresolved import {id:#x} called at {site:#x}, returns 0");
        }
        return 0;
    }
    let mut code = [0x90u8; 6];
    code[0] = 0xE8;
    code[1..5].copy_from_slice(&(function.wrapping_sub(site + 5) as i32).to_le_bytes());
    unsafe { write(site, &code) };
    function
}

unsafe fn write(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const c_void, bytes.len(), old, &mut old);
    }
}

pub(crate) fn init() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let resolver = base + RESOLVER;
    let thunk = unsafe { std::slice::from_raw_parts((base + THUNK) as *const u8, 9) };
    let target = (base + THUNK + 5).wrapping_add(i32::from_le_bytes(thunk[1..5].try_into().unwrap()) as usize);
    let empty = unsafe { std::slice::from_raw_parts(resolver as *const u8, 5) }.iter().all(|&b| b == 0);
    if thunk[0] != 0xE8 || target != resolver || thunk[5..9] != 0x1643a441u32.to_le_bytes() || !empty {
        log!("swarm: import resolver not found, unresolved imports left alone");
        return;
    }
    let mut code = [0xE9u8; 5];
    code[1..5].copy_from_slice(&((import_stub as *const () as usize).wrapping_sub(resolver + 5) as i32).to_le_bytes());
    unsafe { write(resolver, &code) };
    log!("swarm: import resolver at {resolver:#x}");
}
