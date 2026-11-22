//! GRID's LAN discovery, linked (`WAL_LINK`): its cabinets find each other through its
//! discovery server (UDP 12001, its poll `0xA6CA40`), which answers a search only when the
//! search's territory and track pack match its own. Its own are read as none (`0xFFFFFFFF`)
//! while the searches say 0: no server ever answered, each cabinet raced alone. Linked, the
//! server's two values none are made 0 at each poll (the 1.1 executable's code, checked first).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

const POLL: usize = 0xA6_CA40;
/// `sub esp, 0x440`: the poll's first instruction, moved to the trampoline.
const PROLOGUE: [u8; 6] = [0x81, 0xEC, 0x40, 0x04, 0x00, 0x00];

static ORIG_POLL: AtomicUsize = AtomicUsize::new(0);

/// The discovery server (`this`): +8 its state (1 on), +0x14 its socket, +0x18 title,
/// +0x1C version, +0x20 territory, +0x24 track pack.
unsafe extern "fastcall" fn poll(server: *mut u32, edx: usize, arg: i32) -> i32 {
    let s = server;
    unsafe {
        if *s.add(2) == 1 && *s.add(5) != 0 && (*s.add(8) == u32::MAX || *s.add(9) == u32::MAX) {
            static LOGGED: AtomicUsize = AtomicUsize::new(0);
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 3 {
                log!("grid: discovery server: territory {:#x}, track pack {:#x} (none) made 0, as the searches say", *s.add(8), *s.add(9));
            }
            for i in [8, 9] {
                if *s.add(i) == u32::MAX {
                    *s.add(i) = 0;
                }
            }
        }
        let f: unsafe extern "fastcall" fn(*mut u32, usize, i32) -> i32 = std::mem::transmute(ORIG_POLL.load(Ordering::Relaxed));
        f(server, edx, arg)
    }
}

pub(crate) fn init() {
    if !wal_payload_common::link::linked() || !wal_payload_common::patches::exe_name().eq_ignore_ascii_case("GRID.exe") {
        return;
    }
    if unsafe { std::slice::from_raw_parts(POLL as *const u8, PROLOGUE.len()) } != PROLOGUE {
        log!("grid: the discovery server's poll is not 1.1's: left as it is");
        return;
    }
    unsafe {
        // the trampoline: the prologue, then back after it
        let t = VirtualAlloc(std::ptr::null(), 16, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
        if t.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(PROLOGUE.as_ptr(), t, PROLOGUE.len());
        *t.add(6) = 0xE9;
        std::ptr::write_unaligned(t.add(7) as *mut i32, (POLL + PROLOGUE.len()) as i32 - (t as usize + 11) as i32);
        ORIG_POLL.store(t as usize, Ordering::Relaxed);
        let mut old = 0;
        VirtualProtect(POLL as *const _, 6, PAGE_EXECUTE_READWRITE, &mut old);
        *(POLL as *mut u8) = 0xE9;
        std::ptr::write_unaligned((POLL + 1) as *mut i32, poll as *const () as usize as i32 - (POLL + 5) as i32);
        *((POLL + 5) as *mut u8) = 0x90;
        VirtualProtect(POLL as *const _, 6, old, &mut old);
    }
    log!("grid: linked, the discovery server's territory / track pack fixed (0xA6CA40)");
}
