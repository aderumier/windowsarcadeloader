//! Valve Limit R's cabinets linked across PCs (`WAL_LINK`, payload-common `link.rs`): Microsoft's
//! DirectPlay (`dpnet.dll`, the profile's), 4 cabinets, UDP 13400. Its NETWORK-ID (the test menu's,
//! `d:\expdat`, bits 20-22 of its settings word `0x581e40`, less one) chooses: 1 hosts, the
//! others connect to 192.168.64.100 (then the odd ones to .102), the addresses it gives the PCs
//! (written in its code). Linked, its NETWORK-ID is the cabinet's number, set after each read of
//! its settings (`0x40afb0`), and those two addresses are the PCs of cabinets 1 and 3 (the known
//! build checked); DirectPlay's sockets are the link's.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{link, log, patches};

const SETTINGS: usize = 0x581E40;
const READ: usize = 0x40AFB0;
const PROLOGUE: [u8; 12] = [0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF8, 0x81, 0xEC, 0x84, 0x0F, 0x00, 0x00];
/// The two hosts' addresses (16 bytes each, NUL padded).
const HOSTS: [(usize, &[u8; 14], usize); 2] = [(0x4CB248, b"192.168.64.100", 1), (0x4CB258, b"192.168.64.102", 3)];

static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// Its settings' read (`this` call, the backup flag): its NETWORK-ID the link's.
unsafe extern "fastcall" fn read(this: usize, edx: usize, backup: i32) -> i32 {
    let f: unsafe extern "fastcall" fn(usize, usize, i32) -> i32 = unsafe { std::mem::transmute(ORIGINAL.load(Ordering::Relaxed)) };
    let r = unsafe { f(this, edx, backup) };
    let w = SETTINGS as *mut u32;
    unsafe { w.write_volatile((w.read_volatile() & !0x70_0000) | (((link::cabinet() as u32 - 1) & 7) << 20)) };
    r
}

/// Valve Limit R's executable (launcher.exe, as other Type X games): its settings' read and its
/// hosts' addresses where they are expected.
pub(crate) fn is_game() -> bool {
    wal_payload_common::crash::readable(READ, PROLOGUE.len())
        && unsafe { std::slice::from_raw_parts(READ as *const u8, PROLOGUE.len()) } == PROLOGUE
        && HOSTS.iter().all(|(at, text, _)| wal_payload_common::crash::readable(*at, 16) && unsafe { std::slice::from_raw_parts(*at as *const u8, 14) } == *text)
}

pub(crate) fn init() {
    if link::init() == 0 {
        return;
    }
    if link::cabinet() > 4 {
        return log!("vlrlink: Valve Limit R links 4 cabinets: cabinet {} plays alone", link::cabinet());
    }
    let Some(t) = (unsafe { patches::detour(READ, &PROLOGUE, read as *const () as usize) }) else { return };
    ORIGINAL.store(t, Ordering::Relaxed);
    let mut list = Vec::new();
    for (at, _, k) in HOSTS {
        if let Some(a) = link::address(k) {
            let mut b = [0u8; 16];
            let t = link::dotted(a);
            b[..t.len()].copy_from_slice(t.as_bytes());
            list.push(format!("{:#x}:{}", at - 0x400000, b.iter().map(|x| format!("{x:02X}")).collect::<String>()));
        }
    }
    patches::apply_list(&list.join(","));
    link::hook_module_when_loaded("dpnet.dll");
    log!("vlrlink: linked, NETWORK-ID {} ({})", link::cabinet(), if link::cabinet() == 1 { "it hosts" } else { "it joins cabinet 1" });
}
