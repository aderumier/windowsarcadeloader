//! The "Kaboom" Adrio two-feeder I/O board of Spider-Man Homecoming and Crossy Road (the same
//! keypad: the players' jump buttons on up and down):
//! `KaboomAdrioFxPlusTwoFeeders.dll`, its serial board's library, replaced (cdecl, every
//! function a `KABOOM_DLL_ERROR` byte, 0 no error).
//!
//! - **Board.** `KaboomAdrioFxPlusTwoFeeders` opens the first port `InitLib(n, keyboard, coin,
//!   object detected, object not detected, wheel spinner)` (n 2..19) whose coins and feeders
//!   reset and whose feeder counts read.
//! - **Callbacks.** The keypad callback gets the keys held (`un16KeyPressed`: operator 1, down 2,
//!   up 4, select 8), the coin callback the two counters' running totals (the game credits
//!   their increase). The game calls no function every frame: the callbacks come from a payload
//!   thread (started at `InitLib`, Mono loaded) attached to the game's Mono domain.
//! - **Players.** Spider-Man's two players have one button each, the keypad's up (player 1) and
//!   down (player 2); coins: player 1 on counter 1, the others on counter 2.
//! - **Tickets.** `SetTicketFeederTicketToFeedCount(count 1, count 2)` sets the feeders' target
//!   counts, `GetTicketFeederCount` reports the fed counts and empty flags (a ticket every
//!   100 ms up to them, never empty), `ResetTicketFeeder` zeroes both.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::log;
use wal_payload_common::mono::mono;

use crate::{Native, PLAYERS, held};

const NO_ERROR: u8 = 0;

const KEY_OPERATOR: u16 = 1;
const KEY_DOWN: u16 = 2;
const KEY_UP: u16 = 4;
const KEY_SELECT: u16 = 8;

type KeyboardEvent = unsafe extern "C" fn(keys: u16);
type CoinEvent = unsafe extern "C" fn(count1: u16, count2: u16);

static KEYBOARD: AtomicUsize = AtomicUsize::new(0);
static COIN: AtomicUsize = AtomicUsize::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

struct Board {
    /// The two coin counters.
    coins: [u16; 2],
    /// Each feeder's target and fed counts.
    to_feed: [u16; 2],
    fed: [u16; 2],
}

static BOARD: Mutex<Board> = Mutex::new(Board { coins: [0; 2], to_feed: [0; 2], fed: [0; 2] });

/// The keypad's keys held: the operator's, and the players' buttons on up and down.
fn keys() -> u16 {
    let mut k = 0;
    for (native, bit) in [(Native::Operator, KEY_OPERATOR), (Native::Down, KEY_DOWN), (Native::Up, KEY_UP), (Native::Select, KEY_SELECT)] {
        if held(0, native) {
            k |= bit;
        }
    }
    for (player, bit) in [(0, KEY_UP), (1, KEY_DOWN)] {
        if held(player, Native::Button) {
            k |= bit;
        }
    }
    k
}

/// Samples the inputs every 10 ms: the callbacks of their changes; feeds the tickets.
fn board_thread() {
    if !mono().is_some_and(|m| m.attach()) {
        log!("adrio2: cannot attach to Mono: no inputs");
        return;
    }
    let mut last_keys = 0;
    let mut coin_held = [false; PLAYERS];
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let keys = keys();
        if keys != last_keys {
            last_keys = keys;
            let f = KEYBOARD.load(Ordering::Relaxed);
            if f != 0 {
                unsafe { std::mem::transmute::<usize, KeyboardEvent>(f)(keys) };
            }
        }
        let mut coins = None;
        for (p, was) in coin_held.iter_mut().enumerate() {
            let now = held(p, Native::Coin);
            if now && !*was {
                let mut b = BOARD.lock().unwrap();
                let counter = &mut b.coins[p.min(1)];
                *counter = counter.wrapping_add(1);
                coins = Some(b.coins);
                log!("adrio2: coin, player {}", p + 1);
            }
            *was = now;
        }
        if let Some([c1, c2]) = coins {
            let f = COIN.load(Ordering::Relaxed);
            if f != 0 {
                unsafe { std::mem::transmute::<usize, CoinEvent>(f)(c1, c2) };
            }
        }
        tick += 1;
        if tick.is_multiple_of(10) {
            let mut b = BOARD.lock().unwrap();
            for f in 0..2 {
                if b.fed[f] != b.to_feed[f] {
                    b.fed[f] = b.fed[f].wrapping_add(1);
                }
            }
        }
    }
}

unsafe fn write<T>(p: *mut T, v: T) {
    if !p.is_null() {
        unsafe { p.write(v) };
    }
}

/// The functions without results: no error.
extern "C" fn ok() -> u8 {
    NO_ERROR
}

extern "C" fn init_lib(port: i32, keyboard: usize, coin: usize, _detected: usize, _not_detected: usize, _wheel: usize) -> u8 {
    log!("adrio2: InitLib(COM{port}): the board");
    KEYBOARD.store(keyboard, Ordering::Relaxed);
    COIN.store(coin, Ordering::Relaxed);
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(board_thread);
    }
    NO_ERROR
}

extern "C" fn close_lib() {
    log!("adrio2: CloseLib");
}

extern "C" fn get_keypad_count(out: *mut [u16; 4]) -> u8 {
    unsafe { write(out, [0; 4]) };
    NO_ERROR
}

extern "C" fn get_ticket_to_feed(c1: *mut u16, c2: *mut u16) -> u8 {
    let b = BOARD.lock().unwrap();
    unsafe {
        write(c1, b.to_feed[0]);
        write(c2, b.to_feed[1]);
    }
    NO_ERROR
}

extern "C" fn set_ticket_to_feed(c1: u16, c2: u16) -> u8 {
    log!("adrio2: feeders to {c1} and {c2} tickets");
    BOARD.lock().unwrap().to_feed = [c1, c2];
    NO_ERROR
}

extern "C" fn get_ticket_feeder_count(c1: *mut u16, empty1: *mut u16, c2: *mut u16, empty2: *mut u16) -> u8 {
    let b = BOARD.lock().unwrap();
    unsafe {
        write(c1, b.fed[0]);
        write(empty1, 0);
        write(c2, b.fed[1]);
        write(empty2, 0);
    }
    NO_ERROR
}

extern "C" fn reset_ticket_feeder() -> u8 {
    let mut b = BOARD.lock().unwrap();
    b.to_feed = [0; 2];
    b.fed = [0; 2];
    NO_ERROR
}

extern "C" fn get_current_coin_count(c1: *mut u16, c2: *mut u16) -> u8 {
    let b = BOARD.lock().unwrap();
    unsafe {
        write(c1, b.coins[0]);
        write(c2, b.coins[1]);
    }
    NO_ERROR
}

extern "C" fn reset_coin_count(c1: *mut u16, c2: *mut u16) -> u8 {
    BOARD.lock().unwrap().coins = [0; 2];
    unsafe {
        write(c1, 0);
        write(c2, 0);
    }
    NO_ERROR
}

/// `ReadInput`: the keypad's keys at bits 8-11 (enter, down, select, up).
extern "C" fn read_input(out: *mut u16) -> u8 {
    let k = keys();
    let mut bits = 0;
    for (key, bit) in [(KEY_OPERATOR, 0x100), (KEY_DOWN, 0x200), (KEY_SELECT, 0x400), (KEY_UP, 0x800)] {
        if k & key != 0 {
            bits |= bit;
        }
    }
    unsafe { write(out, bits) };
    NO_ERROR
}

extern "C" fn get_event_mask(out: *mut u16) -> u8 {
    unsafe { write(out, 0xFFFF) };
    NO_ERROR
}

extern "C" fn get_ir_detector_count(_output: u16, count: *mut u16) -> u8 {
    unsafe { write(count, 0) };
    NO_ERROR
}

/// `GetSteeringPosition`: centred.
extern "C" fn get_steering_position() -> u8 {
    128
}

/// The board's functions.
pub fn resolve(name: &str) -> Option<usize> {
    let f = match name {
        "InitLib" | "InitLib2Feeder" => init_lib as *const () as usize,
        "CloseLib" => close_lib as *const () as usize,
        "GetKeypadCount" => get_keypad_count as *const () as usize,
        "GetTicketFeederTicketToFeedCount" => get_ticket_to_feed as *const () as usize,
        "SetTicketFeederTicketToFeedCount" => set_ticket_to_feed as *const () as usize,
        "GetTicketFeederCount" => get_ticket_feeder_count as *const () as usize,
        "ResetTicketFeeder" => reset_ticket_feeder as *const () as usize,
        "GetCurrentCoinCount" => get_current_coin_count as *const () as usize,
        "ResetCoinCount" => reset_coin_count as *const () as usize,
        "ReadInput" => read_input as *const () as usize,
        "GetEventMask" => get_event_mask as *const () as usize,
        "GetIRDetectorCount" => get_ir_detector_count as *const () as usize,
        "GetSteeringPosition" => get_steering_position as *const () as usize,
        // event mask, outputs, LED display, diagnostics, steering, IR detectors, wheel spinner
        "SetEventMask" | "SetOutput" | "ClearOutput" | "SetExtendedOutput" | "ClearExtendedOutput" | "SetDiagnosticState" | "SetLedDisplay" | "InitSteeringKickStart"
        | "GetSteeringStatus" | "SetSteeringParameters" | "SetSteeringUpdateRate" | "SetIRDetectorCount" | "StartIRDetector" | "StartWheelSpinner" => {
            ok as *const () as usize
        }
        _ => return None,
    };
    Some(f)
}
