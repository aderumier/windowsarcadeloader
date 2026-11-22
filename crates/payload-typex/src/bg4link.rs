//! Battle Gear 4's cabinets linked across PCs (`WAL_LINK`, payload-common `link.rs`), both
//! editions (4 cabinets, the editions apart: their packets' magic differ). Each cabinet
//! broadcasts its state (UDP 8821, every frame), must hear its own, and opens TCP 9573 to the
//! others' addresses read in those packets. Its MACHINE NO (the test menu's NETWORK SETTING,
//! `D:\config.bin`) is the link's cabinet number, set after each read of its settings (Tuned
//! `0x51c980`, the first edition `0x4e4870`, the known builds checked).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{link, log, patches};

struct Build {
    load: usize,
    prologue: [u8; 8],
    machine: usize,
    copy: usize,
}

const TUNED: Build = Build { load: 0x51C980, prologue: [0x53, 0x56, 0x57, 0x68, 0xA0, 0xCD, 0x76, 0x00], machine: 0x8D0B34, copy: 0x8DEB0C };
const FIRST: Build = Build { load: 0x4E4870, prologue: [0x55, 0x56, 0x57, 0x68, 0x68, 0x09, 0x73, 0x00], machine: 0x82FAAC, copy: 0x83DA8C };

static LOAD: AtomicUsize = AtomicUsize::new(0);
static MACHINE: AtomicUsize = AtomicUsize::new(0);
static COPY: AtomicUsize = AtomicUsize::new(0);

/// Its settings' read (`this` call): MACHINE NO the link's.
unsafe extern "fastcall" fn load(this: usize, edx: usize) -> i32 {
    let f: unsafe extern "fastcall" fn(usize, usize) -> i32 = unsafe { std::mem::transmute(LOAD.load(Ordering::Relaxed)) };
    let r = unsafe { f(this, edx) };
    let k = link::cabinet() - 1;
    unsafe {
        (MACHINE.load(Ordering::Relaxed) as *mut u16).write_volatile(k as u16);
        (COPY.load(Ordering::Relaxed) as *mut u32).write_volatile(k as u32);
    }
    r
}

/// `game`: the profile's `WAL_TYPEX_FFB` (`battle-gear-4`: Tuned, `battle-gear-4-original`).
pub(crate) fn init(game: &str) {
    if link::init() == 0 {
        return;
    }
    if link::cabinet() > 4 {
        return log!("bg4link: Battle Gear 4 links 4 cabinets at most: cabinet {} plays alone", link::cabinet());
    }
    let b = if game == "battle-gear-4" { TUNED } else { FIRST };
    MACHINE.store(b.machine, Ordering::Relaxed);
    COPY.store(b.copy, Ordering::Relaxed);
    match unsafe { patches::detour(b.load, &b.prologue, load as *const () as usize) } {
        Some(t) => {
            LOAD.store(t, Ordering::Relaxed);
            log!("bg4link: linked, MACHINE NO {}", link::cabinet());
        }
        None => log!("bg4link: not the known build: MACHINE NO left to its test menu"),
    }
}
