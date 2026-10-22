//! Otomedius: the payload is the game's `libacio.dll` (ACIO bus, card reader, PJEI board,
//! `pjei.rs`); otomedius.exe also imports `libbms.dll` (the BM-System client, `bms.rs`) and
//! `libtouchpanel.dll` (the touch screen, `touchpanel.rs`). Those imports are pointed at this
//! DLL's functions of the same names: the game's own DLLs stay loaded, unused, and one payload
//! instance serves the game (one launcher connection, one input state).

use std::ffi::{CStr, c_void};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// Writes `new` at `at` when the code there is `expected`.
unsafe fn patch(what: &str, at: usize, expected: &[u8], new: &[u8]) {
    let current = unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) };
    if current != expected {
        log!("otomedius: {what}: code differs ({current:02x?}), not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const c_void, new.len(), old, &mut old);
    }
    log!("otomedius: {what}");
}

/// This DLL's exported functions: (name, address).
fn exports(module: *mut c_void) -> Vec<(String, usize)> {
    let base = module as usize;
    let read = |offset: usize| unsafe { ((base + offset) as *const u32).read_unaligned() } as usize;
    let pe = read(0x3c);
    // the optional header's first data directory (exports) of a PE32 image
    let dir = read(pe + 0x78);
    if dir == 0 {
        return Vec::new();
    }
    let (count, functions, names, ordinals) = (read(dir + 0x18), read(dir + 0x1c), read(dir + 0x20), read(dir + 0x24));
    (0..count)
        .map(|i| {
            let name = unsafe { CStr::from_ptr((base + read(names + i * 4)) as *const _) }.to_string_lossy().into_owned();
            let ordinal = unsafe { ((base + ordinals + i * 2) as *const u16).read_unaligned() } as usize;
            (name, base + read(functions + ordinal * 4))
        })
        .collect()
}

pub fn init(module: *mut c_void) {
    let mut hooked = [0usize; 2];
    for (name, address) in exports(module) {
        let (dll, slot) = if name.starts_with("bms_") {
            ("libbms.dll", 0)
        } else if name.starts_with("ac_touchpanel_") {
            ("libtouchpanel.dll", 1)
        } else {
            continue;
        };
        // `hook` logs each import it replaces; most of ours are not imported
        if unsafe { iat::hook(dll, &name, address) }.is_some() {
            hooked[slot] += 1;
        }
    }
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    // The boot's data check (step 0) reads `ChkSum.bin`, the data files and their hashes, which
    // dumps lack: FILE BROKEN 5-1504-0000. Without the file the step is passed: its handler
    // becomes `mov byte [step], 1; jmp <handled>` (a present file is still checked).
    if !std::path::Path::new("ChkSum.bin").exists() {
        let handler = exe + 0x2d492;
        let rel = (exe + 0x2db81).wrapping_sub(handler + 12) as u32;
        let mut code = vec![0xc6, 0x05, 0x27, 0x20, 0x5e, 0x00, 0x01, 0xe9];
        code.extend_from_slice(&rel.to_le_bytes());
        unsafe { patch("data check without ChkSum.bin passed", handler, &[0x83, 0x7e, 0x18, 0x00, 0x8d, 0x7e, 0x18, 0x75, 0x2a], &code) };
    }
    log!("otomedius: {} libbms and {} libtouchpanel imports answered by the payload", hooked[0], hooked[1]);
}
