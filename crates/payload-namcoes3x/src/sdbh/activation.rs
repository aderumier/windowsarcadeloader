//! The activation card: the operator's card of this release (a stock of plays bought from the
//! distributor, read by a reader on the I/O board's serial channel). The game links its
//! library (`BngSecCard*`): its functions are replaced by a card always in stock. Without it
//! the attract screen shows error 28-1 (activation card reader).
//!
//! The library's calls (cdecl): `Init(4, path)` and `Decrement(n)` start a command (its id,
//! more than 0), `CheckCommand(id)` is not 0 when it is done, `GetError()` (the game checks
//! `& 0x1f0007`), `GetNum()` the cards, `GetType(i)` (1: an activation card),
//! `GetQuantity(i)` its stock, `Exec(dt)` the library's step (its serial traffic).

use std::sync::atomic::{AtomicI32, Ordering};

use wal_payload_common::log;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The library's functions in this build (addresses of the executable at its preferred base).
const FUNCTIONS: [(usize, &str); 8] = [
    (0xbdb100, "Init"),
    (0xbdb1f0, "Exit"),
    (0xbdb330, "Decrement"),
    (0xbdb5d0, "GetNum"),
    (0xbdb5e0, "GetType"),
    (0xbdb620, "GetQuantity"),
    (0xbdb6d0, "CheckCommand"),
    (0xbdb750, "GetError"),
];
const EXEC: usize = 0xbdc460;
const PREFERRED_BASE: usize = 0x400000;

/// The stock the card shows.
const STOCK: i32 = 9999;

static NEXT_COMMAND: AtomicI32 = AtomicI32::new(1);

unsafe fn jump(at: usize, to: usize) {
    let mut code = [0xE9u8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&(to.wrapping_sub(at + 5) as i32).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, 5);
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
}

pub fn init(exe: usize) {
    let hooks: [usize; 8] = [
        init_card as *const () as usize,
        exit as *const () as usize,
        decrement as *const () as usize,
        get_num as *const () as usize,
        get_type as *const () as usize,
        get_quantity as *const () as usize,
        check_command as *const () as usize,
        get_error as *const () as usize,
    ];
    for ((va, _), hook) in FUNCTIONS.iter().zip(hooks) {
        unsafe { jump(exe + va - PREFERRED_BASE, hook) };
    }
    unsafe { jump(exe + EXEC - PREFERRED_BASE, exec as *const () as usize) };
    log!("sdbh: activation card: in stock ({STOCK} plays)");
}

fn command() -> i32 {
    NEXT_COMMAND.fetch_add(1, Ordering::Relaxed).max(1)
}

unsafe extern "C" fn init_card(_kind: i32, _path: usize) -> i32 {
    command()
}
unsafe extern "C" fn exit() {}
unsafe extern "C" fn decrement(n: i32) -> i32 {
    log!("sdbh: activation card: {n} play(s) used");
    command()
}
unsafe extern "C" fn get_num() -> i32 {
    1
}
unsafe extern "C" fn get_type(_card: i32) -> i32 {
    1
}
unsafe extern "C" fn get_quantity(_card: i32) -> i32 {
    STOCK
}
unsafe extern "C" fn check_command(_id: i32) -> i32 {
    1
}
unsafe extern "C" fn get_error() -> i32 {
    0
}
unsafe extern "C" fn exec(_dt: f32) {}
