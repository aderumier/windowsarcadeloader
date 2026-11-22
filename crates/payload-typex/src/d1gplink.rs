//! D1GP Arcade's cabinets linked across PCs (`WAL_LINK`, payload-common `link.rs`): Microsoft's
//! DirectPlay (`dpnet.dll`, the profile's), 2 cabinets, UDP 55555, the client finding the host by
//! a broadcast. Alone, the profile's patches keep its link task from hosting (0x84dd0, 0x855b3,
//! 0x856f0, the standalone cabinet's): linked, they are undone, and who hosts is the link's
//! (its test menu's CONNECTION METHOD, `+0x5e` of its settings at `0x629086`, 0 HOST / 1 CLIENT,
//! set after each read of its settings `0x4a4120` and their factory values `0x4a3fa0`, and held
//! (Mode Select offers LINK BATTLE when the other cabinet announces the other method); the jump to
//! the client's search `0x484cb9` never taken on cabinet 1, always on cabinet 2, whose own port is
//! left to DirectPlay `0x484b6e`); DirectPlay's sockets are the link's (the known build checked).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{link, log, patches};

const METHOD: usize = 0x629086;
static READ: AtomicUsize = AtomicUsize::new(0);
static FACTORY: AtomicUsize = AtomicUsize::new(0);

fn method() {
    unsafe { (METHOD as *mut i16).write_volatile(if link::cabinet() == 1 { 0 } else { 1 }) };
}

unsafe extern "C" fn read() -> i32 {
    let f: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(READ.load(Ordering::Relaxed)) };
    let r = unsafe { f() };
    method();
    r
}

unsafe extern "C" fn factory() {
    let f: unsafe extern "C" fn() = unsafe { std::mem::transmute(FACTORY.load(Ordering::Relaxed)) };
    unsafe { f() };
    method();
}

pub(crate) fn init() {
    if link::init() == 0 {
        return;
    }
    if link::cabinet() > 2 {
        return log!("d1gplink: D1GP links 2 cabinets: cabinet {} plays alone", link::cabinet());
    }
    let hooks: [(usize, &[u8], usize, &AtomicUsize); 2] = [
        (0x4A4120, &[0x81, 0xEC, 0x04, 0x01, 0x00, 0x00], read as *const () as usize, &READ),
        (0x4A3FA0, &[0x53, 0x68, 0x98, 0x00, 0x00, 0x00], factory as *const () as usize, &FACTORY),
    ];
    for (at, prologue, hook, slot) in hooks {
        match unsafe { patches::detour(at, prologue, hook) } {
            Some(t) => slot.store(t, Ordering::Relaxed),
            None => return log!("d1gplink: not the known build ({at:#x}): alone"),
        }
    }
    // the standalone patches undone (the link task hosts, draws), who hosts the link's
    let mut list = String::from("0x84DD0:833DD0ED610005,0x855B3:0F8712010000,0x856F0:A1D0ED");
    list.push_str(if link::cabinet() == 1 { ",0x84CB9:9090" } else { ",0x84CB9:EB,0x84B6E:EB3E" });
    patches::apply_list(&list);
    method();
    // held too while Mode Select checks its conditions (LINK BATTLE: the other cabinet announces
    // the other method; the factory values had put HOST back on a fresh save)
    std::thread::spawn(|| loop {
        method();
        std::thread::sleep(std::time::Duration::from_millis(50));
    });
    link::hook_module_when_loaded("dpnet.dll");
    log!("d1gplink: linked, cabinet {} ({})", link::cabinet(), if link::cabinet() == 1 { "HOST" } else { "CLIENT" });
}
