//! Team Play's I/O library (`WAL_ARCADEPC_BOARD=teamplay`): Family Guy Bowling Arcade, a Linux
//! cabinet game (Unity 5.4, Mono) run with Unity's Windows player. Its `DllImport`s of the
//! cabinet's `libteamplayio.so` and `libserialio.so` (no Windows build: stand-in handles,
//! `mono_imports::stand_in`) are the payload's functions:
//!
//! - **Switches** (`IO_GetInputState`, the board's input bits): start `0x400`, left `0x200`,
//!   right `0x800`, select `0x1000`, the operator's adjustables `0x40` (test) and service credit
//!   `0x8` (service). The game makes its edges itself (`TeamPlayIO.Update`).
//! - **The trackball** (`IO_SpinnerGetCurrentValue`, spinners 8 = x, 9 = y, read once a frame):
//!   the counts since the last read of the mouse's wrapping counters (`mouse_wrap` on `rx` /
//!   `ry`), times `WAL_TEAMPLAY_TRACKBALL` (default 1), plus the left stick's tilt (up to
//!   `WAL_TEAMPLAY_STICK` counts a frame, default 45: the back swing wants 27 a frame, the throw 14 for 8 frames). The game turns spinner 8 the other way:
//!   both signs make a roll to the right / forward the game's +x / +y.
//! - **Coins** (`IO_DefineCoinMech`'s mechs, `IO_NewCoinCount`, `IO_CashAvailable`,
//!   `IO_UseCash`): cash in cents, each coin its mech's cents (25 by default); the coin input is
//!   mech 0.
//! - Security (`IO_IsSecure`: secure, no cabinet flags: not linked, not home mode), board
//!   connected, versions, the time string; outputs, tickets, comm links, the printer's serial
//!   port and the log are accepted and dropped.
//!
//! Free play: the game's `Adjustables` file (`key|value|...`, and its `.bak`), installed by the
//! profile, gets `freePlayOn` from the common free play (`WAL_FREEPLAY`) before the game reads it.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, mono_imports};
use wal_protocol::Axis;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A switch of the board's input state, its bit.
    Switch(u32),
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Switch(0x400)),
    ("left", Native::Switch(0x200)),
    ("right", Native::Switch(0x800)),
    ("select", Native::Switch(0x1000)),
    ("adjustables", Native::Switch(0x40)),
    ("service_credit", Native::Switch(0x8)),
    ("coin", Native::Coin),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "select"),
    ("test", "adjustables"),
    ("service", "service_credit"),
    ("coin", "coin"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The coin mechs: (cents a coin, cash in cents).
static MECHS: Mutex<[(u32, u32); 3]> = Mutex::new([(25, 0); 3]);
/// Coins not yet reported by `IO_NewCoinCount`, per mech.
static NEW_COINS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
/// The coin input held at the last read.
static COIN_HELD: AtomicU32 = AtomicU32::new(0);
/// The mouse counters at the last read of spinners 8 and 9.
static LAST_MOUSE: [AtomicI32; 2] = [const { AtomicI32::new(i32::MIN) }; 2];

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// The switches held, a coin counted on its press.
unsafe extern "C" fn get_input_state(_board: u32) -> u32 {
    let mut state = 0;
    let mut coin = false;
    map().for_each_pressed(&input(0), |n| match n {
        Native::Switch(bit) => state |= bit,
        Native::Coin => coin = true,
    });
    if coin && COIN_HELD.swap(1, Ordering::Relaxed) == 0 {
        NEW_COINS[0].fetch_add(1, Ordering::Relaxed);
        let mut mechs = MECHS.lock().unwrap();
        mechs[0].1 += mechs[0].0;
        log!("teamplay: coin, cash {} cents", mechs[0].1);
    } else if !coin {
        COIN_HELD.store(0, Ordering::Relaxed);
    }
    state
}

/// Trackball counts since the last read of `spinner` (8: x, 9: y), plus the stick's tilt.
unsafe extern "C" fn spinner_get_current_value(_board: u32, spinner: u32) -> i32 {
    let (i, axis, stick_axis) = match spinner {
        8 => (0, Axis::RightX, Axis::LeftX),
        9 => (1, Axis::RightY, Axis::LeftY),
        _ => return 0,
    };
    let stick = input(0);
    let now = stick.axis(axis);
    let was = LAST_MOUSE[i].swap(now as i32, Ordering::Relaxed);
    let mouse = if was == i32::MIN { 0.0 } else { now.wrapping_sub(was as i16) as f32 };
    let tilt = stick.axis(stick_axis) as f32 / 32767.0;
    let tilt = if tilt.abs() < 0.15 { 0.0 } else { tilt };
    let counts = (mouse * env_f32("WAL_TEAMPLAY_TRACKBALL", 1.0) + tilt * env_f32("WAL_TEAMPLAY_STICK", 45.0)).round() as i32;
    // a roll right / forward: the mouse's and the stick's +x / -y; the game's +x is spinner 8
    // negated, its +y spinner 9
    -counts
}

unsafe extern "C" fn define_coin_mech(_board: u32, idx: u32, _input: u32, cents_per_coin: u32, cash: u32, _meter: i32) {
    if let Some(m) = MECHS.lock().unwrap().get_mut(idx as usize) {
        *m = (cents_per_coin.max(1), m.1.max(cash));
        log!("teamplay: coin mech {idx}: {cents_per_coin} cents a coin");
    }
}

/// `board` is the mech here (the game's calls give `NewCoinCount(0)`, `(1)`, `(2)`).
unsafe extern "C" fn new_coin_count(board: u32, idx: i32) -> u32 {
    let mech = if idx >= 0 { idx as usize } else { board as usize };
    NEW_COINS.get(mech).map_or(0, |c| c.swap(0, Ordering::Relaxed))
}

unsafe extern "C" fn cash_available(_board: u32, idx: i32) -> u32 {
    let mechs = MECHS.lock().unwrap();
    if idx >= 0 { mechs.get(idx as usize).map_or(0, |m| m.1) } else { mechs.iter().map(|m| m.1).sum() }
}

unsafe extern "C" fn use_cash(_board: u32, count: u32, idx: i32) -> bool {
    let mut mechs = MECHS.lock().unwrap();
    let mut left = count;
    for (i, m) in mechs.iter_mut().enumerate() {
        if idx >= 0 && i != idx as usize {
            continue;
        }
        let used = left.min(m.1);
        m.1 -= used;
        left -= used;
    }
    left == 0
}

unsafe extern "C" fn is_secure(_source: *const u8, flags: *mut u32) -> bool {
    if !flags.is_null() {
        unsafe { *flags = 0 };
    }
    true
}

unsafe extern "C" fn yes() -> bool {
    true
}

unsafe extern "C" fn serial_number(_board: u32) -> i32 {
    1
}

/// A version into the game's `StringBuilder` (8 chars).
unsafe extern "C" fn version(_board: u32, out: *mut u8) {
    if !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(b"1.0\0".as_ptr(), out, 4) };
    }
}

/// The local time into the game's `StringBuilder` (80 chars).
unsafe extern "C" fn time_string(out: *mut u8) {
    if out.is_null() {
        return;
    }
    let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    let s = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}\0", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), out, s.len()) };
}

/// Everything else: accepted, nothing (0, false).
unsafe extern "C" fn ignored() -> usize {
    0
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module != "teamplayio.dll" && module != "serialio.dll" {
        return None;
    }
    Some(match name {
        "IO_GetInputState" => get_input_state as *const () as usize,
        "IO_SpinnerGetCurrentValue" | "IO_SpinnerGetAverageValue" => spinner_get_current_value as *const () as usize,
        "IO_DefineCoinMech" => define_coin_mech as *const () as usize,
        "IO_NewCoinCount" => new_coin_count as *const () as usize,
        "IO_CashAvailable" => cash_available as *const () as usize,
        "IO_UseCash" => use_cash as *const () as usize,
        "IO_IsSecure" => is_secure as *const () as usize,
        "IO_IsConnected" | "SerialOpenPort" => yes as *const () as usize,
        "IO_GetSerialNumber" => serial_number as *const () as usize,
        "IO_GetVersion" | "IO_GetHWVersion" | "IO_GetDeviceInfo" => version as *const () as usize,
        "IO_GetTimeString" => time_string as *const () as usize,
        _ => ignored as *const () as usize,
    })
}

/// `freePlayOn` of the game's `Adjustables` files from the common free play.
fn free_play() {
    let Some(free) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") else { return };
    for file in ["Adjustables", "Adjustables.bak"] {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let mut fields: Vec<String> = text.split('|').map(str::to_string).collect();
        let Some(i) = fields.iter().position(|f| f == "freePlayOn") else { continue };
        if let Some(v) = fields.get_mut(i + 1) {
            *v = if free { "1" } else { "0" }.into();
        }
        let out = fields.join("|");
        if out != text && std::fs::write(file, out).is_ok() {
            log!("teamplay: {file} freePlayOn = {}", free as u8);
        }
    }
}

pub fn init() {
    free_play();
    mono_imports::stand_in(&["teamplayio.dll", "serialio.dll"]);
    mono_imports::hook(resolve);
    log!("teamplay: Team Play I/O library (teamplayio, serialio)");
}
