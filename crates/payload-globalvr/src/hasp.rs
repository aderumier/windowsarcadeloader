//! HASP4 dongle: the game's `hasp()` calls are answered by [`hook`].
//!
//! Far Cry Paradise Lost links the HASP4 API statically: `hasp(service, seed, lpt, pw1, pw2,
//! &p1, &p2, &p3, &p4)` (cdecl) at `0x8a9094`, called from 4 places (IsHasp, HaspStatus,
//! ReadBlock of the 112-byte memory, HaspID). The calls are repointed to [`hook`] in memory, so
//! the executable stays untouched.
//!
//! The game reads the dongle at boot and when a level starts, and keeps bytes 0x00..0x13 of its
//! memory in a struct at `0xa5278c` (`tools/dongle-recording/farcry-dongle-dump.py` reads it back
//! from a running game). Byte 0x0f is the cabinet type (`0x4bd9d0`): '0' / '1' / '2' switch the
//! cabinet mode, '1' puts the player's camera at water level from the first level on, where the
//! player stays stuck; 0 keeps the default mode.

use std::ffi::c_void;

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// `hasp()` and its call sites, as image offsets (image base 0x400000).
const HASP: usize = 0x4a9094;
const CALL_SITES: [usize; 4] = [0x4a8def, 0x4a8e1e, 0x4a8e76, 0x4a8f60];

const IS_HASP: i32 = 1;
const HASP_STATUS: i32 = 5;
const HASP_ID: i32 = 6;
const READ_BLOCK: i32 = 50;

/// Dongle memory: game id, version, country, region, cabinet type, cabinet; the rest of the
/// 112 bytes reads 0xff.
const MEMORY: &[u8; 0x14] = b"PARLST1.0CTRYUS\0CAB1";
/// HaspID, only shown in the operator screens.
const ID: u32 = 0x5741_4c00;

/// Repoints the game's `hasp()` calls to [`hook`]; leaves the game alone if they are not
/// where expected (other build).
pub(crate) fn init() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    for site in CALL_SITES {
        let at = base + site;
        let code = unsafe { std::slice::from_raw_parts(at as *const u8, 5) };
        let rel = i32::from_le_bytes(code[1..5].try_into().unwrap());
        if code[0] != 0xE8 || (at + 5).wrapping_add(rel as usize) != base + HASP {
            log!("hasp: call site {site:#x} not found, dongle not emulated");
            return;
        }
    }
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
    _seed: i32,
    _lpt: i32,
    _pw1: i32,
    _pw2: i32,
    p1: *mut i32,
    p2: *mut i32,
    p3: *mut i32,
    p4: *mut i32,
) {
    // the game tries two password pairs; any is accepted
    let out = match service {
        IS_HASP => [1, 0x100, 0, 0],
        HASP_STATUS => [1, 1, 1, 0],
        HASP_ID => [(ID >> 16) as i32, (ID & 0xffff) as i32, 0, 0],
        READ_BLOCK => {
            // p1 start word, p2 words, p4 buffer
            let (start, words, buf) = unsafe { (*p1 as usize, *p2 as usize, *p4 as *mut u8) };
            for i in 0..words * 2 {
                let b = MEMORY.get(start * 2 + i).copied().unwrap_or(0xff);
                unsafe { *buf.add(i) = b };
            }
            [unsafe { *p1 }, unsafe { *p2 }, 0, unsafe { *p4 }]
        }
        _ => [0, 0, 0, 0],
    };
    unsafe { [*p1, *p2, *p3, *p4] = out };
    log!("hasp: service {service} -> {out:?}");
}
