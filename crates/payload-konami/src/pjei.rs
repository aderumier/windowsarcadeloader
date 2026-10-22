//! The PJEI I/O board (Otomedius): switches, coin mech, lamps, security plugs.
//!
//! `ac_io_pjei_get_control_status_buffer` fills 40 bytes; the game reads byte 4 (start 0x80, up
//! 0x40, down 0x20, left 0x10, right 0x08, buttons A 0x04, B 0x02, C 0x01) and byte 16 (test
//! 0x20, service 0x10, coin mech 0x04), the layout spice2x documents for Winning Eleven's PJEI.
//! Coins are a stock (`ac_io_pjei_current_coinstock`) the game takes and consumes.
//!
//! Native inputs for `native_map`: `test service coin start up down left right a b c`.

use std::sync::Mutex;

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    C,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
    ("a", Native::A),
    ("b", Native::B),
    ("c", Native::C),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "a"),
    ("b2", "b"),
    ("b3", "c"),
];

/// (status byte, bit) of a switch.
fn switch(native: Native) -> (usize, u8) {
    match native {
        Native::Start => (4, 0x80),
        Native::Up => (4, 0x40),
        Native::Down => (4, 0x20),
        Native::Left => (4, 0x10),
        Native::Right => (4, 0x08),
        Native::A => (4, 0x04),
        Native::B => (4, 0x02),
        Native::C => (4, 0x01),
        Native::Test => (16, 0x20),
        Native::Service => (16, 0x10),
        Native::Coin => (16, 0x04),
    }
}

struct Board {
    map: ButtonMap<Native>,
    status: [u8; 40],
    coin_held: bool,
    coins: u32,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

fn with_board<R>(f: impl FnOnce(&mut Board) -> R) -> R {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board { map: ButtonMap::new(NATIVES, DEFAULT_MAP), status: [0; 40], coin_held: false, coins: 0 });
    f(board)
}

/// A new status from player 1's stick.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_update_control_status_buffer() -> bool {
    with_board(|b| {
        let stick = wal_payload_common::input(0);
        let mut status = [0u8; 40];
        let mut coin = false;
        b.map.for_each_pressed(&stick, |n| {
            let (byte, bit) = switch(n);
            status[byte] |= bit;
            coin |= n == Native::Coin;
        });
        if coin && !b.coin_held {
            b.coins += 1;
        }
        b.coin_held = coin;
        b.status = status;
    });
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_get_control_status_buffer(out: *mut u8) -> bool {
    if out.is_null() {
        return false;
    }
    with_board(|b| unsafe { std::ptr::copy_nonoverlapping(b.status.as_ptr(), out, b.status.len()) });
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_current_coinstock(_slot: u32, out: *mut u32) -> bool {
    if !out.is_null() {
        unsafe { *out = with_board(|b| b.coins) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_consume_coinstock(_slot: u32, count: u32) -> bool {
    with_board(|b| b.coins = b.coins.saturating_sub(count));
    true
}

// Security plugs: the game's (`GEGGGJAA`) found at once (status 0x14), none missing (8).

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_req_secplug_check(id: *const std::ffi::c_char) -> bool {
    if !id.is_null() {
        log!("pjei: software plug {} present", unsafe { std::ffi::CStr::from_ptr(id) }.to_string_lossy());
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_req_secplug_check_isfinished(status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 0x14 };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_req_secplug_missing_check() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_req_secplug_missing_check_isfinished(status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 8 };
    }
    true
}

/// A plug id: `0140` and 16 hex digits (20 characters and the NUL).
fn plug_id(out: *mut u8, id: &[u8; 8]) -> bool {
    if out.is_null() {
        return false;
    }
    let mut text = String::from("0140");
    for b in id {
        text.push_str(&format!("{b:02X}"));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), out, text.len());
        *out.add(text.len()) = 0;
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_get_softwareid(out: *mut u8) -> bool {
    plug_id(out, b"GEGGGJAA")
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_get_systemid(out: *mut u8) -> bool {
    plug_id(out, &[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_get_watchdog_status() -> u32 {
    0
}

// Coin counters, blockers and lamps: outputs without a device here.

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_lock_coincounter(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_unlock_coincounter(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_control_coin_blocker_on(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_control_coin_blocker_off(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_control_lamp_bright(_lamp: u32, _brightness: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjei_control_lamp_mode(_mode: u32) -> bool {
    true
}
