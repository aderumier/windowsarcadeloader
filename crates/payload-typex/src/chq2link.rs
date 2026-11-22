//! Chase H.Q. 2's cabinets linked across PCs (`WAL_LINK`, payload-common `link.rs`): Microsoft's
//! DirectPlay (`dpnet.dll`, the profile's), 2 cabinets, UDP 13400. Its network ID (bits 17-18 of
//! its settings word `0x170bf28`) chooses at its network check (`0x465bf0`): 0 hosts the session,
//! the others connect to `chq2.ini`'s HOSTADDR (192.168.65.110 by default). Linked, `chq2.ini`'s
//! LINKHOST (its number reader `0x401fa0`) is 1 on cabinet 1 alone, HOSTADDR (its text reader
//! `0x401f30`) cabinet 1's PC, the network ID the cabinet's number less one, and DirectPlay's
//! sockets are the link's (the known build checked).

use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{link, log, patches};

const SETTINGS: usize = 0x170BF28;
static SETTING: AtomicUsize = AtomicUsize::new(0);
static TEXT: AtomicUsize = AtomicUsize::new(0);
static NETWORK: AtomicUsize = AtomicUsize::new(0);
static HOST: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();

fn named(name: *const c_char, wanted: &str) -> bool {
    !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes() == wanted.as_bytes()
}

/// `chq2.ini`'s number settings: LINKHOST the link's (cabinet 1 hosts).
unsafe extern "C" fn setting(name: *const c_char, value: *mut i32) -> i32 {
    if named(name, "LINKHOST") && !value.is_null() {
        unsafe { *value = (link::cabinet() == 1) as i32 };
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

/// Its network check: the network ID the cabinet's number less one.
unsafe extern "fastcall" fn network(this: usize, edx: usize) -> i32 {
    let w = SETTINGS as *mut u32;
    unsafe { w.write_volatile((w.read_volatile() & !0x60000) | (((link::cabinet() as u32 - 1) & 3) << 17)) };
    let f: unsafe extern "fastcall" fn(usize, usize) -> i32 = unsafe { std::mem::transmute(NETWORK.load(Ordering::Relaxed)) };
    let r = unsafe { f(this, edx) };
    log!("chq2link: network check -> {r} ({})", match r { 2 => "connected to the host", 1 => "hosting", _ => "no session" });
    r
}

pub(crate) fn init() {
    if link::init() == 0 {
        return;
    }
    if link::cabinet() > 2 {
        return log!("chq2link: Chase H.Q. 2 links 2 cabinets: cabinet {} plays alone", link::cabinet());
    }
    if let Some(a) = link::address(1).or_else(|| (link::cabinet() == 1).then(|| link::own()).flatten()) {
        let mut h = [0u8; 16];
        let t = link::dotted(a);
        h[..t.len()].copy_from_slice(t.as_bytes());
        let _ = HOST.set(h);
    }
    let hooks: [(usize, &[u8], usize, &AtomicUsize); 3] = [
        (0x401FA0, &[0x53, 0x8B, 0x5C, 0x24, 0x0C], setting as *const () as usize, &SETTING),
        (0x401F30, &[0x55, 0x8B, 0x6C, 0x24, 0x08], text as *const () as usize, &TEXT),
        (0x465BF0, &[0x83, 0xEC, 0x14, 0xA1, 0x60, 0x72, 0x5D, 0x00], network as *const () as usize, &NETWORK),
    ];
    for (at, prologue, hook, slot) in hooks {
        match unsafe { patches::detour(at, prologue, hook) } {
            Some(t) => slot.store(t, Ordering::Relaxed),
            None => return log!("chq2link: not the known build ({at:#x}): left to its test menu"),
        }
    }
    link::hook_module_when_loaded("dpnet.dll");
    log!("chq2link: linked, cabinet {} ({})", link::cabinet(), if link::cabinet() == 1 { "it hosts" } else { "it joins cabinet 1" });
}
