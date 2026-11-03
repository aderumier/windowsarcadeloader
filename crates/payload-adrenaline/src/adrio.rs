//! The "Kaboom" Adrio I/O board of Rabbids Hollywood, Tomb Raider and Rampage
//! (`KaboomAdrioFxPlusFourFeeders64.dll`) and Spider-Man Homecoming (`KaboomAdrioFxPlusFourFeeders.dll`, 32-bit): its serial board's
//! library, replaced (cdecl, every function a `KABOOM_DLL_ERROR` byte, 0 no error).
//!
//! - **Board.** `KaboomAdrioFxPlusFourFeeders` opens the first port `InitLib(n, callbacks)` (n
//!   2..19) whose `GetTargetVersion` is 9 or more and whose feeders' counts are 0 after a reset;
//!   a firmware revision other than 1102 is an error popup. The board reports through the
//!   callbacks given to `InitLib`: the keypad (`un16AllKeys`: operator 1, down 2, select 4, up
//!   8), the coin counters (a structure by value: its address; four doors of two counters,
//!   running totals: the game credits their increase) and the player buttons (`un16AllButton`:
//!   player n's button 1 (start) at bit 2n, button 2 at 2n + 1: the gun's trigger, which
//!   Rabbids reads besides the tracker's button and Tomb Raider only there).
//! - **Callbacks.** Rabbids' board thread polls `GetTicketFeederCountAdrio5` after every Unity
//!   frame: the payload samples the inputs and calls the callbacks there, from the game's
//!   managed thread. Rampage's calls nothing every frame: a payload thread (started at
//!   `InitLib`, Mono loaded) attached to the game's Mono domain samples them every 10 ms too.
//! - **Rampage.** Player n's joystick on the player buttons' bits 4n (down 1, up 2, left 4,
//!   right 8); the attack buttons on the keypad: player 1 down, 2 select, 3 up (operator 1).
//! - **Tickets.** `SetTicketFeederCountToFeedAdrio5` sets a feeder's target count (`{ feeder,
//!   count }` by value), `GetTicketFeederCountAdrio5` reports the fed counts (a ticket every
//!   100 ms up to it, the feeders never empty); `ResetTicketFeederAdrio5` (a feeder mask)
//!   zeroes both.
//! - **Polled (Spider-Man).** The older library takes no callbacks: the game polls
//!   `GetReadInputAdrio5` (`{ buttons, optos and coins, keypad }`, active low: a cleared bit is
//!   pressed; coins: door n's coin 1 at bit 4 + 2n) every frame and on a coin's press reads the
//!   door's counter (`GetCurrentCoinCountAdrio5`) plus one: the counter before that coin. The
//!   two players' buttons are the keypad's up (player 1) and down (player 2).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use wal_payload_common::log;

use crate::{Native, PLAYERS, held};

const NO_ERROR: u8 = 0;
/// The board's controller version and the firmware revision the game expects.
const VERSION: u16 = 9;
const REVISION: u16 = 1102;

const KEY_OPERATOR: u16 = 1;
const KEY_DOWN: u16 = 2;
const KEY_SELECT: u16 = 4;
const KEY_UP: u16 = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CoinCounters {
    input1: u16,
    input2: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FeededTickets {
    count: [u16; 4],
    all_empty: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PlayerButtonInfo {
    all_flags: u16,
    counts: [u16; 8],
}

type KeyboardEvent = unsafe extern "C" fn(keys: u16);
type CoinEvent = unsafe extern "C" fn(doors: *const [CoinCounters; 4]);
type PlayerButtonEvent = unsafe extern "C" fn(buttons: u16);

/// `InitLib`'s keypad, coin and player button callbacks.
static KEYBOARD: AtomicUsize = AtomicUsize::new(0);
static COIN: AtomicUsize = AtomicUsize::new(0);
static PLAYER_BUTTON: AtomicUsize = AtomicUsize::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

struct Board {
    /// The coin doors' totals.
    coins: [u16; PLAYERS],
    coin_held: [bool; PLAYERS],
    keys: u16,
    buttons: u16,
    /// Each feeder's target and fed counts, and when it fed its last ticket.
    to_feed: [u16; 4],
    fed: [u16; 4],
    last_feed: Option<Instant>,
}

static BOARD: Mutex<Board> = Mutex::new(Board {
    coins: [0; PLAYERS],
    coin_held: [false; PLAYERS],
    keys: 0,
    buttons: 0,
    to_feed: [0; 4],
    fed: [0; 4],
    last_feed: None,
});

/// The keypad's keys held (Spider-Man: the players' buttons on up and down).
fn keys() -> u16 {
    let operator = [(Native::Operator, KEY_OPERATOR), (Native::Down, KEY_DOWN), (Native::Select, KEY_SELECT), (Native::Up, KEY_UP)]
        .iter()
        .filter(|(n, _)| held(0, *n))
        .fold(0, |k, (_, bit)| k | bit);
    let players = [(0, KEY_UP), (1, KEY_DOWN)].iter().filter(|(p, _)| held(*p, Native::Button)).fold(0, |k, (_, bit)| k | bit);
    // Rampage's attack buttons
    let attacks = [(0, KEY_DOWN), (1, KEY_SELECT), (2, KEY_UP)].iter().filter(|(p, _)| held(*p, Native::Attack)).fold(0, |k, (_, bit)| k | bit);
    operator | players | attacks
}

/// The player buttons held: start (button 1, bit 2n), the gun's trigger (button 2, bit
/// 2n + 1), Rampage's joysticks (bits 4n).
fn buttons() -> u16 {
    let start = (0..PLAYERS).filter(|&p| held(p, Native::Start)).fold(0, |b, p| b | 1 << (2 * p));
    let trigger = (0..PLAYERS).filter(|&p| held(p, Native::Trigger)).fold(0, |b, p| b | 1 << (2 * p + 1));
    let mut sticks = 0;
    for p in 0..3 {
        for (native, bit) in [(Native::StickDown, 1), (Native::StickUp, 2), (Native::StickLeft, 4), (Native::StickRight, 8)] {
            if held(p, native) {
                sticks |= bit << (4 * p);
            }
        }
    }
    start | trigger | sticks
}

/// Samples the inputs every 10 ms from a thread attached to Mono (Rampage).
fn board_thread() {
    if !wal_payload_common::mono::mono().is_some_and(|m| m.attach()) {
        log!("adrio: cannot attach to Mono: inputs from the game's calls only");
        return;
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        poll();
    }
}

/// Samples the inputs: the callbacks of their changes (the game's managed thread).
fn poll() {
    let (keys_now, buttons_now) = (keys(), buttons());
    let mut fire_keys = None;
    let mut fire_buttons = None;
    let mut fire_coins = None;
    {
        let mut b = BOARD.lock().unwrap();
        if keys_now != b.keys {
            b.keys = keys_now;
            fire_keys = Some(keys_now);
        }
        if buttons_now != b.buttons {
            b.buttons = buttons_now;
            fire_buttons = Some(buttons_now);
        }
        let mut coined = false;
        for p in 0..PLAYERS {
            let now = held(p, Native::Coin);
            if now && !b.coin_held[p] {
                b.coins[p] = b.coins[p].wrapping_add(1);
                coined = true;
                log!("adrio: coin, player {}", p + 1);
            }
            b.coin_held[p] = now;
        }
        if coined {
            let mut doors = [CoinCounters::default(); 4];
            for (door, &count) in doors.iter_mut().zip(&b.coins) {
                door.input1 = count;
            }
            fire_coins = Some(doors);
        }
        // a ticket every 100 ms
        let due = b.last_feed.is_none_or(|t| t.elapsed().as_millis() >= 100);
        if due {
            let mut fed = false;
            for f in 0..4 {
                if b.fed[f] != b.to_feed[f] {
                    b.fed[f] = b.fed[f].wrapping_add(1);
                    fed = true;
                }
            }
            b.last_feed = fed.then(Instant::now);
        }
    }
    unsafe {
        if let Some(k) = fire_keys {
            let f = KEYBOARD.load(Ordering::Relaxed);
            if f != 0 {
                std::mem::transmute::<usize, KeyboardEvent>(f)(k);
            }
        }
        if let Some(b) = fire_buttons {
            let f = PLAYER_BUTTON.load(Ordering::Relaxed);
            if f != 0 {
                std::mem::transmute::<usize, PlayerButtonEvent>(f)(b);
            }
        }
        if let Some(doors) = fire_coins {
            let f = COIN.load(Ordering::Relaxed);
            if f != 0 {
                std::mem::transmute::<usize, CoinEvent>(f)(&doors);
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

extern "C" fn init_lib(port: i32, keyboard: usize, coin: usize, _ticket: usize, player_button: usize, _spinner: usize) -> u8 {
    log!("adrio: InitLib(COM{port}): the board");
    KEYBOARD.store(keyboard, Ordering::Relaxed);
    COIN.store(coin, Ordering::Relaxed);
    PLAYER_BUTTON.store(player_button, Ordering::Relaxed);
    if (keyboard | coin | player_button) != 0 && !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(board_thread);
    }
    NO_ERROR
}

extern "C" fn close_lib() {
    log!("adrio: CloseLib");
}

extern "C" fn get_target_version(version: *mut [u16; 2]) -> u8 {
    unsafe { write(version, [VERSION, REVISION]) };
    NO_ERROR
}

/// `GetInvalidTargetVersion`: a bool, false.
extern "C" fn get_invalid_target_version(_version: *mut [u16; 2]) -> i32 {
    0
}

extern "C" fn get_ticket_feeder_count(out: *mut FeededTickets) -> u8 {
    poll();
    let fed = BOARD.lock().unwrap().fed;
    unsafe { write(out, FeededTickets { count: fed, all_empty: 0 }) };
    NO_ERROR
}

/// `SetTicketFeederCountToFeedAdrio5({ feeder, count })`: the structure by value.
extern "C" fn set_ticket_to_feed(feed: u32) -> u8 {
    let (feeder, count) = ((feed & 0xFFFF) as usize, (feed >> 16) as u16);
    if feeder < 4 {
        log!("adrio: feeder {} to {count} tickets", feeder + 1);
        BOARD.lock().unwrap().to_feed[feeder] = count;
    }
    NO_ERROR
}

extern "C" fn get_ticket_to_feed(feeder: u16, out: *mut [u16; 2]) -> u8 {
    let count = BOARD.lock().unwrap().to_feed.get(feeder as usize).copied().unwrap_or(0);
    unsafe { write(out, [feeder, count]) };
    NO_ERROR
}

/// `ResetTicketFeederAdrio5(mask)`: the feeders' counts zeroed.
extern "C" fn reset_ticket_feeder(mask: u16) -> u8 {
    let mut b = BOARD.lock().unwrap();
    for f in 0..4 {
        if mask & 1 << f != 0 {
            b.to_feed[f] = 0;
            b.fed[f] = 0;
        }
    }
    NO_ERROR
}

/// A door's counter; while its coin is held, before that coin (the polling game adds it).
extern "C" fn get_current_coin_count(door: u16, out: *mut CoinCounters) -> u8 {
    let b = BOARD.lock().unwrap();
    let door = door as usize;
    let count = b.coins.get(door).map_or(0, |&c| c.wrapping_sub(b.coin_held[door] as u16));
    unsafe { write(out, CoinCounters { input1: count, input2: 0 }) };
    NO_ERROR
}

/// `ResetCoinCountAdrio5(mask)`: two bits a door.
extern "C" fn reset_coin_count(mask: u16) -> u8 {
    let mut b = BOARD.lock().unwrap();
    for door in 0..PLAYERS {
        if mask & 3 << (2 * door) != 0 {
            b.coins[door] = 0;
        }
    }
    NO_ERROR
}

/// `GetReadInputAdrio5`: buttons, optos and coins, keypad; active low.
extern "C" fn get_read_input(out: *mut [u16; 3]) -> u8 {
    poll();
    let b = BOARD.lock().unwrap();
    let coins = (0..PLAYERS).filter(|&p| b.coin_held[p]).fold(0u16, |c, p| c | 1 << (4 + 2 * p));
    unsafe { write(out, [!b.buttons, !coins, !b.keys]) };
    NO_ERROR
}

extern "C" fn get_event_mask(out: *mut u16) -> u8 {
    unsafe { write(out, 0xFFFF) };
    NO_ERROR
}

extern "C" fn get_player_button_status(out: *mut PlayerButtonInfo) -> u8 {
    let buttons = BOARD.lock().unwrap().buttons;
    unsafe { write(out, PlayerButtonInfo { all_flags: buttons, counts: [0; 8] }) };
    NO_ERROR
}

extern "C" fn get_motor_pwm(_motor: u8, duty: *mut u8) -> u8 {
    unsafe { write(duty, 0) };
    NO_ERROR
}

/// The board's functions.
pub fn resolve(name: &str) -> Option<usize> {
    let f = match name {
        "InitLib" => init_lib as *const () as usize,
        "CloseLib" => close_lib as *const () as usize,
        "GetTargetVersion" => get_target_version as *const () as usize,
        "GetInvalidTargetVersion" => get_invalid_target_version as *const () as usize,
        "GetTicketFeederCountAdrio5" => get_ticket_feeder_count as *const () as usize,
        "SetTicketFeederCountToFeedAdrio5" => set_ticket_to_feed as *const () as usize,
        "GetTicketFeederCountToFeedAdrio5" => get_ticket_to_feed as *const () as usize,
        "ResetTicketFeederAdrio5" => reset_ticket_feeder as *const () as usize,
        "GetCurrentCoinCountAdrio5" => get_current_coin_count as *const () as usize,
        "ResetCoinCountAdrio5" => reset_coin_count as *const () as usize,
        "GetReadInputAdrio5" => get_read_input as *const () as usize,
        "GetEventMaskAdrio5" => get_event_mask as *const () as usize,
        "GetPlayerButtonStatus" => get_player_button_status as *const () as usize,
        "GetMotorPwmDutyCycle" => get_motor_pwm as *const () as usize,
        // reset, event masks, lights, coin outputs, motors
        "ResetTarget" | "SetEventMaskAdrio5" | "ClrEventMaskAdrio5" | "ResetPlayerButtonCount" | "SetPlayerButtonLight" | "ClrPlayerButtonLight" | "SetCoinOutput"
        | "ClrCoinOutput" | "SetMotorPwmDutyCycle" | "StopMotorPwm" => ok as *const () as usize,
        _ => return None,
    };
    Some(f)
}
