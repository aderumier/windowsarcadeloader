//! The "Kaboom" I/O board (`KaboomComDll.dll`, its serial board) of Skylanders Cloud Patrol.
//!
//! - **Board.** `KaboomMgr` opens the first port `InitLib(n)` (n 0..9) answering
//!   `GetCurrentCoinCount`, then polls (every 0.25 s) the two coin counters (more than the last
//!   poll: a coin; `ResetCoinCount` after crediting) and the keypad's four press counters (up,
//!   down, menu, select: any change opens or closes the operator menu). Every function returns
//!   a `KABOOM_DLL_ERROR` byte, 0 no error (cdecl). Coins: player 1's on counter 1, the others'
//!   on counter 2.
//! - **Tickets.** Redemption mode: the results add the tickets won to the dispenser's "to
//!   feed" count (`SetTicketFeederTicketToFeedCount`), then poll the fed count
//!   (`GetTicketFeederCount`: fed, empty) until it reaches it and reset both
//!   (`ResetTicketFeeder`). The dispenser feeds a ticket every 100 ms, never empty.
//! - **Jetpack Joyride Arcade.** Resolves all the library's functions (`GetProcAddress`, any
//!   missing: no board) and opens the board with `InitLib(port, keypad, coins, tickets, x)`:
//!   cdecl callbacks the board thread calls, the keypad's `(keys)` on a change (operator 1, down
//!   2, select 4, up 8: any of the last three is the jetpack button, `button` sets 2), the coins' `(counter 1, counter 2)` running totals on a coin (the game
//!   credits their increase; `ResetCoinCount` zeroes them). It also uses the dispenser and the
//!   lamps (`SetOutput`, `ClearOutput`, `ClearExtendedOutput`: one u16 of lamp bits, ignored);
//!   the others answer no error.
//! - **Serial (Infinity Blade FX).** The board library compiled into the game (`CKaboomCom`):
//!   the same functions as packets on the board's port (`\\.\COMn`, the first answering): a
//!   request `F10E`, checksum, length, command, data; the reply `B00F`, checksum (16-bit sum of
//!   the length, command and data bytes), length, the command, its data (u16 little-endian
//!   fields). The game's commands: 6 keypad (18 bytes: the four press counts at 12..16),
//!   8 / 9 get / set the tickets to feed, 10 the fed count and the empty flag, 11 reset the
//!   feeder, 12 / 13 the coin counters (and reset them).

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::log;

use crate::{Native, PLAYERS, held};

const NO_ERROR: u8 = 0;

/// The board's two coin counters.
static COINS: [AtomicU16; 2] = [const { AtomicU16::new(0) }; 2];
/// The keypad's "menu" press counter (the operator's key).
static MENU: AtomicU16 = AtomicU16::new(0);
/// The dispenser's tickets to feed, and fed.
static TO_FEED: AtomicU16 = AtomicU16::new(0);
static FED: AtomicU16 = AtomicU16::new(0);
static OPENED: AtomicBool = AtomicBool::new(false);
/// Jetpack Joyride's `InitLib` callbacks: the keypad's and the coins'.
static KEYPAD_CALLBACK: AtomicUsize = AtomicUsize::new(0);
static COIN_CALLBACK: AtomicUsize = AtomicUsize::new(0);
/// `InitLib` takes the callbacks (Jetpack Joyride), not the port only.
pub static WITH_CALLBACKS: AtomicBool = AtomicBool::new(false);

/// Counts the coin and operator key presses, feeds the tickets.
fn board_thread() {
    let mut coin_was = [false; PLAYERS];
    let mut operator_was = false;
    let mut keys_was = 0u16;
    let mut feed_tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let mut coined = false;
        for (player, was) in coin_was.iter_mut().enumerate() {
            let now = held(player, Native::Coin);
            if now && !*was {
                COINS[player.min(1)].fetch_add(1, Ordering::Relaxed);
                coined = true;
            }
            *was = now;
        }
        let coin_callback = COIN_CALLBACK.load(Ordering::Relaxed);
        if coined && coin_callback != 0 {
            let f: extern "C" fn(u32, u32) = unsafe { std::mem::transmute(coin_callback) };
            f(COINS[0].load(Ordering::Relaxed) as u32, COINS[1].load(Ordering::Relaxed) as u32);
        }
        let keypad_callback = KEYPAD_CALLBACK.load(Ordering::Relaxed);
        if keypad_callback != 0 {
            let keys = [(Native::Operator, 1), (Native::Down, 2), (Native::Button, 2), (Native::Select, 4), (Native::Up, 8)]
                .iter()
                .filter(|(n, _)| held(0, *n))
                .fold(0u16, |k, (_, bit)| k | bit);
            if keys != keys_was {
                keys_was = keys;
                let f: extern "C" fn(u32) = unsafe { std::mem::transmute(keypad_callback) };
                f(keys as u32);
            }
        }
        let now = held(0, Native::Operator);
        if now && !operator_was {
            MENU.fetch_add(1, Ordering::Relaxed);
        }
        operator_was = now;
        feed_tick += 1;
        if feed_tick >= 10 {
            feed_tick = 0;
            let (fed, to_feed) = (FED.load(Ordering::Relaxed), TO_FEED.load(Ordering::Relaxed));
            if fed < to_feed {
                FED.store(fed + 1, Ordering::Relaxed);
                log!("kaboom: ticket {} of {to_feed}", fed + 1);
            }
        }
    }
}

/// `p`, written if not null.
unsafe fn put(p: *mut u16, v: u16) {
    if !p.is_null() {
        unsafe { *p = v };
    }
}

extern "C" fn init_lib(port: i32) -> u8 {
    if !OPENED.swap(true, Ordering::Relaxed) {
        log!("kaboom: InitLib({port}): the board");
    }
    NO_ERROR
}

/// Jetpack Joyride's `InitLib(port, keypad, coins, tickets, x)`: the board on the first port.
extern "C" fn init_lib_callbacks(port: u32, keypad: usize, coins: usize, _tickets: usize, _x: usize) -> u8 {
    KEYPAD_CALLBACK.store(keypad, Ordering::Relaxed);
    COIN_CALLBACK.store(coins, Ordering::Relaxed);
    if !OPENED.swap(true, Ordering::Relaxed) {
        log!("kaboom: InitLib({port}) with callbacks: the board");
    }
    NO_ERROR
}

extern "C" fn close_lib() {}

extern "C" fn check_comm() -> u8 {
    NO_ERROR
}

extern "C" fn get_current_coin_count(c1: *mut u16, c2: *mut u16) -> u8 {
    unsafe {
        put(c1, COINS[0].load(Ordering::Relaxed));
        put(c2, COINS[1].load(Ordering::Relaxed));
    }
    NO_ERROR
}

extern "C" fn reset_coin_count(c1: *mut u16, c2: *mut u16) -> u8 {
    unsafe {
        put(c1, COINS[0].swap(0, Ordering::Relaxed));
        put(c2, COINS[1].swap(0, Ordering::Relaxed));
    }
    NO_ERROR
}

/// `KaboomKeypad`: the up, down, menu and select keys' press counts.
extern "C" fn get_keypad_count(keypad: *mut [u16; 4]) -> u8 {
    if !keypad.is_null() {
        unsafe { *keypad = [0, 0, MENU.load(Ordering::Relaxed), 0] };
    }
    NO_ERROR
}

extern "C" fn get_ticket_to_feed(count: *mut u16) -> u8 {
    unsafe { put(count, TO_FEED.load(Ordering::Relaxed)) };
    NO_ERROR
}

extern "C" fn set_ticket_to_feed(count: u16) -> u8 {
    log!("kaboom: {count} tickets to feed");
    TO_FEED.store(count, Ordering::Relaxed);
    NO_ERROR
}

extern "C" fn get_ticket_feeder_count(fed: *mut u16, empty: *mut u16) -> u8 {
    unsafe {
        put(fed, FED.load(Ordering::Relaxed));
        put(empty, 0);
    }
    NO_ERROR
}

extern "C" fn reset_ticket_feeder() -> u8 {
    TO_FEED.store(0, Ordering::Relaxed);
    FED.store(0, Ordering::Relaxed);
    NO_ERROR
}

extern "C" fn reset_empty_ticket_feeder(count: *mut u16) -> u8 {
    unsafe { put(count, 0) };
    NO_ERROR
}

/// The lamps (`SetOutput`, `ClearOutput`, `SetExtendedOutput`, `ClearExtendedOutput`: lamp
/// bits), the steering, IR detector and event functions Jetpack Joyride resolves but does not
/// use: no error (cdecl: the arguments are the caller's to pop).
extern "C" fn no_error() -> u8 {
    NO_ERROR
}

/// `ReadInput(inputs)`: the board's inputs, none.
extern "C" fn read_input(inputs: *mut u16) -> u8 {
    unsafe { put(inputs, 0) };
    NO_ERROR
}

/// The board's functions.
pub fn resolve(name: &str) -> Option<usize> {
    let f = match name {
        "InitLib" if WITH_CALLBACKS.load(Ordering::Relaxed) => init_lib_callbacks as *const () as usize,
        "InitLib" => init_lib as *const () as usize,
        "CloseLib" => close_lib as *const () as usize,
        "CheckComm" => check_comm as *const () as usize,
        "GetCurrentCoinCount" => get_current_coin_count as *const () as usize,
        "ResetCoinCount" => reset_coin_count as *const () as usize,
        "GetKeypadCount" => get_keypad_count as *const () as usize,
        "GetTicketFeederTicketToFeedCount" => get_ticket_to_feed as *const () as usize,
        "SetTicketFeederTicketToFeedCount" => set_ticket_to_feed as *const () as usize,
        "GetTicketFeederCount" => get_ticket_feeder_count as *const () as usize,
        "ResetTicketFeeder" => reset_ticket_feeder as *const () as usize,
        "ResetEmptyTicketFeeder" => reset_empty_ticket_feeder as *const () as usize,
        "ReadInput" => read_input as *const () as usize,
        "SetOutput" | "ClearOutput" | "SetExtendedOutput" | "ClearExtendedOutput" | "GetEventMask" | "SetEventMask" | "SetDiagnosticState"
        | "InitSteeringKickStart" | "GetSteeringStatus" | "SetSteeringParameters" | "SetSteeringUpdateRate" | "GetSteeringPosition"
        | "GetIRDetectorCount" | "SetIRDetectorCount" | "StartIRDetector" => no_error as *const () as usize,
        _ => return None,
    };
    Some(f)
}

pub fn init() {
    std::thread::spawn(board_thread);
    log!("kaboom: Kaboom board");
}

/// The board on its serial port (any COM port the game tries): the game's own copy of the
/// library.
pub fn init_serial() {
    wal_payload_common::serial::install("COM*", serial_packet);
    log!("kaboom: Kaboom board on the serial port");
}

const REQUEST: u16 = 0xF10E;
const REPLY: u16 = 0xB00F;

/// Answers a request packet.
fn serial_packet(request: &[u8]) -> Vec<u8> {
    let u16_at = |i: usize| request.get(i..i + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    if request.len() < 8 || u16_at(0) != REQUEST {
        log!("kaboom: not a request: {request:02X?}");
        return Vec::new();
    }
    let command = u16_at(6);
    let data = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let body = match command {
        // the keypad: the menu key's press count (byte 14) toggles the operator menu
        6 => {
            let mut d = vec![0; 18];
            d[14] = MENU.load(Ordering::Relaxed) as u8;
            d
        }
        8 => data(&[TO_FEED.load(Ordering::Relaxed)]),
        9 => {
            set_ticket_to_feed(u16_at(8));
            Vec::new()
        }
        10 => data(&[FED.load(Ordering::Relaxed), 0]),
        11 => {
            reset_ticket_feeder();
            Vec::new()
        }
        12 => data(&[COINS[0].load(Ordering::Relaxed), COINS[1].load(Ordering::Relaxed)]),
        13 => data(&[COINS[0].swap(0, Ordering::Relaxed), COINS[1].swap(0, Ordering::Relaxed)]),
        // the board's version (Fruit Ninja FX 2 opens the first port answering it): two words
        24 => data(&[2, 2]),
        // the library's others (unused by the game): the request's data echoed
        _ => {
            log!("kaboom: serial command {command}");
            request[8..].to_vec()
        }
    };
    let mut reply = Vec::with_capacity(8 + body.len());
    reply.extend(REPLY.to_le_bytes());
    reply.extend([0, 0]);
    reply.extend((body.len() as u16).to_le_bytes());
    reply.extend(command.to_le_bytes());
    reply.extend(&body);
    let sum = reply[4..].iter().fold(0u16, |s, &b| s.wrapping_add(b as u16));
    reply[2..4].copy_from_slice(&sum.to_le_bytes());
    reply
}
