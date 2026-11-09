//! A packed 32-bit executable (CHUNITHM SUPERSTAR's `chuniApp.exe`, `amdaemon.exe`): its file
//! holds a loader stub and the encrypted program, which the stub decrypts into the image at
//! start, then jumps to the program's entry point (`push eax; ret`). The payload's hooks
//! (imports, code patches) need the decrypted program: they wait for that jump.
//!
//! `WAL_PACKED_JUMP=<rva>`: the stub's last instructions (`85 C0 74 02 50 C3 C2 0C 00`: test eax,
//! eax; je; push eax; ret; ret 0xc), replaced by a jump to [`stub`], which runs the payload's
//! deferred start once (the program decrypted), then goes on as they did.
//! `WAL_PACKED_DUMP=<file>`: the decrypted image (`SizeOfImage` bytes from the base) written
//! there first, for its analysis.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS, VirtualProtect, VirtualQuery};

use crate::log;

/// The stub's original last instructions.
const TAIL: [u8; 9] = [0x85, 0xC0, 0x74, 0x02, 0x50, 0xC3, 0xC2, 0x0C, 0x00];

static DEFERRED: OnceLock<fn()> = OnceLock::new();

// The stub's tail with the payload's start first: eax is the program's entry point (0: the
// `ret 0xc` path of its other callers).
#[cfg(target_arch = "x86")]
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "test eax, eax",
    "jz 2f",
    "pushad",
    "push eax",
    "call {after}",
    "add esp, 4",
    "popad",
    "push eax",
    "ret",
    "2:",
    "ret 0xc",
    name = sym packed_stub,
    after = sym after_unpack,
);

#[cfg(target_arch = "x86")]
unsafe extern "C" {
    fn packed_stub();
}

/// The program decrypted (its entry point `entry`): the image dumped (`WAL_PACKED_DUMP`), then
/// the payload's deferred start, once.
extern "C" fn after_unpack(entry: usize) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    // the memory holding the entry point (the image, or the program's own allocation)
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    unsafe { VirtualQuery(entry as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    let region = info.AllocationBase as usize;
    let dumped = std::env::var("WAL_PACKED_DUMP").ok().map(|path| dump(region, path.trim()));
    if let Some(start) = DEFERRED.get() {
        start();
    }
    // the log is up now (the payload's start)
    log!("packed: program decrypted, entry point {entry:#x} (image {base:#x}, its memory {region:#x})");
    if let Some(d) = dumped {
        log!("packed: {d}");
    }
}

/// The memory from `region` (an allocation: its committed regions in a row) written to `path`.
fn dump(region: usize, path: &str) -> String {
    let mut data = Vec::new();
    let mut at = region;
    loop {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { VirtualQuery(at as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 || info.AllocationBase as usize != region {
            break;
        }
        let size = info.RegionSize;
        if info.State == MEM_COMMIT && info.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 {
            data.extend_from_slice(unsafe { std::slice::from_raw_parts(at as *const u8, size) });
        } else {
            data.resize(data.len() + size, 0);
        }
        at += size;
    }
    match std::fs::write(path, &data) {
        Ok(()) => format!("memory at {region:#x} ({:#x} bytes) written to {path}", data.len()),
        Err(e) => format!("{path}: {e}"),
    }
}

/// `WAL_PACKED_JUMP` set: `start` runs once the program is decrypted (true; the payload's start
/// deferred); false: not a packed program, the payload starts now.
pub fn defer(start: fn()) -> bool {
    let Some(rva) = std::env::var("WAL_PACKED_JUMP").ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()) else {
        return false;
    };
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let at = base + rva;
    let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, TAIL.len()) };
    if bytes != TAIL {
        log!("packed: +{rva:#x} is {bytes:02x?}, not the stub's tail: not deferred");
        return false;
    }
    #[cfg(target_arch = "x86")]
    unsafe {
        let mut old = 0;
        VirtualProtect(at as *const c_void, TAIL.len(), PAGE_EXECUTE_READWRITE, &mut old);
        let target = packed_stub as *const () as usize;
        *(at as *mut u8) = 0xE9;
        std::ptr::write_unaligned((at + 1) as *mut i32, target.wrapping_sub(at + 5) as i32);
        for i in 5..TAIL.len() {
            *((at + i) as *mut u8) = 0x90;
        }
        VirtualProtect(at as *const c_void, TAIL.len(), old, &mut old);
    }
    #[cfg(not(target_arch = "x86"))]
    {
        let _ = (PAGE_EXECUTE_READWRITE, VirtualProtect as usize);
        log!("packed: 32-bit stubs only");
        return false;
    }
    let _ = DEFERRED.set(start);
    log!("packed: the payload starts once the program is decrypted (+{rva:#x})");
    true
}
