//! HASP4 dongle: the game's `hasp()` calls go through [`hook`].
//!
//! Far Cry Paradise Lost links the HASP4 API statically: `hasp(service, seed, lpt, pw1, pw2,
//! &p1, &p2, &p3, &p4)` (cdecl) at `0x8a9094`, called from 4 places (IsHasp, HaspStatus,
//! ReadBlock of the 112-byte memory, HaspID). The calls are repointed to [`hook`] in memory, so
//! the executable stays untouched (WindowsLoader checks its CRC).
//!
//! Built with the `record` feature, the hook calls the real `hasp()` (WindowsLoader's emulation
//! answers it) and appends each call to `hasplog.bin` in the current directory:
//! `"HSP4" | service seed lpt pw1 pw2 | p1 p2 p3 p4 before | p1 p2 p3 p4 after |
//! ReadBlock (50): the p2 words read at p4`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// `hasp()` and its call sites, as image offsets (image base 0x400000).
const HASP: usize = 0x4a9094;
const CALL_SITES: [usize; 4] = [0x4a8def, 0x4a8e1e, 0x4a8e76, 0x4a8f60];

type HaspFn = unsafe extern "C" fn(i32, i32, i32, i32, i32, *mut i32, *mut i32, *mut i32, *mut i32);

static REAL: AtomicUsize = AtomicUsize::new(0);

/// Repoints the game's `hasp()` calls to [`hook`]; leaves the game alone if they are not
/// where expected (other build).
pub(crate) fn init() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let target = base + HASP;
    for site in CALL_SITES {
        let at = base + site;
        let code = unsafe { std::slice::from_raw_parts(at as *const u8, 5) };
        let rel = i32::from_le_bytes(code[1..5].try_into().unwrap());
        if code[0] != 0xE8 || (at + 5).wrapping_add(rel as usize) != target {
            log!("hasp: call site {site:#x} not found, dongle hook not installed");
            return;
        }
    }
    REAL.store(target, Ordering::Relaxed);
    for site in CALL_SITES {
        let at = base + site;
        let rel = (hook as *const () as usize).wrapping_sub(at + 5) as i32;
        let mut old = 0;
        unsafe {
            VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::write_unaligned((at + 1) as *mut i32, rel);
            VirtualProtect(at as *const c_void, 5, old, &mut old);
        }
    }
    log!("hasp: {} hasp() calls hooked", CALL_SITES.len());
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn hook(
    service: i32,
    seed: i32,
    lpt: i32,
    pw1: i32,
    pw2: i32,
    p1: *mut i32,
    p2: *mut i32,
    p3: *mut i32,
    p4: *mut i32,
) {
    let real: HaspFn = unsafe { std::mem::transmute(REAL.load(Ordering::Relaxed)) };
    // ReadBlock's buffer address is p4 on input; the answer may overwrite p4
    let input = unsafe { [*p1, *p2, *p3, *p4] };
    unsafe { real(service, seed, lpt, pw1, pw2, p1, p2, p3, p4) };
    let out = unsafe { [*p1, *p2, *p3, *p4] };
    log!("hasp: service {service} seed {seed:#x} pw {pw1:#x}/{pw2:#x} {input:?} -> {out:?}");
    #[cfg(feature = "record")]
    record([service, seed, lpt, pw1, pw2], input, out);
}

#[cfg(feature = "record")]
fn record(args: [i32; 5], input: [i32; 4], out: [i32; 4]) {
    use std::io::Write;
    let mut rec = b"HSP4".to_vec();
    for v in args.iter().chain(input.iter()).chain(out.iter()) {
        rec.extend_from_slice(&v.to_le_bytes());
    }
    // ReadBlock: p1 = start word, p2 = words, p4 = buffer
    if args[0] == 50 && input[3] != 0 {
        let len = (input[1].clamp(0, 256) * 2) as usize;
        rec.extend_from_slice(unsafe { std::slice::from_raw_parts(input[3] as *const u8, len) });
    }
    match std::fs::OpenOptions::new().create(true).append(true).open("hasplog.bin") {
        Ok(mut f) => {
            let _ = f.write_all(&rec);
        }
        Err(e) => log!("hasp: cannot write hasplog.bin: {e}"),
    }
}
