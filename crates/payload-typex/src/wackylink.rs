//! Wacky Races' cabinets linked across PCs (`WAL_LINK`, payload-common `link.rs`): Microsoft's
//! DirectPlay (`dpnet.dll`, the profile's), up to 4 cabinets, UDP 13405, session "wracesnet". Its
//! network ID (`wr.ini`'s LINKID, else its test menu's NETWORK-ID, less one) chooses: 0 hosts, the
//! others connect to HOSTADDR (192.168.66.110 by default, the address it gives the PC). Linked,
//! `wr.ini`'s LINKID (its number reader `0x403390`) is the cabinet's number less one and HOSTADDR
//! (its text reader `0x403320`) cabinet 1's PC (the known build checked); DirectPlay's sockets
//! are the link's. Alone, `wr.ini` and the test menu stay the game's.

use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{link, log, patches};

static SETTING: AtomicUsize = AtomicUsize::new(0);
static TEXT: AtomicUsize = AtomicUsize::new(0);
static HOST: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();

fn named(name: *const c_char, wanted: &str) -> bool {
    !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes() == wanted.as_bytes()
}

/// `wr.ini`'s number settings: LINKID the link's.
unsafe extern "C" fn setting(name: *const c_char, value: *mut i32) -> i32 {
    if named(name, "LINKID") && !value.is_null() {
        unsafe { *value = link::cabinet() as i32 - 1 };
        return 1;
    }
    let f: unsafe extern "C" fn(*const c_char, *mut i32) -> i32 = unsafe { std::mem::transmute(SETTING.load(Ordering::Relaxed)) };
    unsafe { f(name, value) }
}

/// Its text settings: HOSTADDR cabinet 1's PC.
unsafe extern "C" fn text(name: *const c_char) -> *const c_char {
    if named(name, "HOSTADDR") {
        if let Some(h) = HOST.get() {
            return h.as_ptr().cast();
        }
    }
    let f: unsafe extern "C" fn(*const c_char) -> *const c_char = unsafe { std::mem::transmute(TEXT.load(Ordering::Relaxed)) };
    unsafe { f(name) }
}

pub(crate) fn init() {
    if link::init() == 0 {
        return;
    }
    if link::cabinet() > 4 {
        return log!("wackylink: Wacky Races links 4 cabinets: cabinet {} plays alone", link::cabinet());
    }
    if let Some(a) = link::address(1) {
        let mut h = [0u8; 16];
        let t = link::dotted(a);
        h[..t.len()].copy_from_slice(t.as_bytes());
        let _ = HOST.set(h);
    }
    let hooks: [(usize, &[u8], usize, &AtomicUsize); 2] = [
        (0x403390, &[0x53, 0x8B, 0x5C, 0x24, 0x0C], setting as *const () as usize, &SETTING),
        (0x403320, &[0x55, 0x8B, 0x6C, 0x24, 0x08], text as *const () as usize, &TEXT),
    ];
    for (at, prologue, hook, slot) in hooks {
        match unsafe { patches::detour(at, prologue, hook) } {
            Some(t) => slot.store(t, Ordering::Relaxed),
            None => return log!("wackylink: not the known build ({at:#x}): left to its test menu"),
        }
    }
    link::hook_module_when_loaded("dpnet.dll");
    log!("wackylink: linked, NETWORK-ID {} ({})", link::cabinet(), if link::cabinet() == 1 { "it hosts" } else { "it joins cabinet 1" });
}
