//! The "gameport" I/O DLL of the Mrcq / ridong cabinets (`gameport.dll`, MFC, its serial board
//! behind a C++ object): Mech Tornado (32-bit), Fire Hero (64-bit) (`WAL_ARCADEPC_BOARD=gameport`).
//!
//! The game's `dllInstance` imports its flat C API (`DllImport("gameport")`, cdecl thunks onto
//! the object `GetInstance` returns): `init(dll, 1)` = 1 and `GetComErr` false open the board;
//! per player the coins buffered (`GetBufferCoin<n>p`, `SubBufferCoin<n>p` when credited), the
//! start key and the gun's two buttons (`Get<P>Frie1/2`: one shoots, both the bomb / big
//! water), the gun's aim as 10-bit ADC readings (`GetWadc<n>pX/Y`, `Get<P>WadcX/Y` with four
//! players; 0 is ignored); the operator's setting, left, right and ticket keys; the cabinet's
//! outputs (lamps, ticket printers, motors, water pump and valves): accepted and ignored.
//!
//! Mono resolves them (`mono_imports.rs`) to the payload's: no board, no serial port.
//!
//! Aim: x 1 + 1022 * x, y 1 + 1022 * (1 - y) (Unity's y up), the gun's place on the picture.
//! The game maps the reading through four calibration corners per gun (the ini's `gun`, `sgun`
//! and `sdgun` points: top left, top right, bottom left, bottom right): the dump's are (1, 1023),
//! (1023, 1023), (1, 1), (1023, 1).
//!
//! Natives: `start fire1 fire2 coin` (each player), `setting left right ticket` (the
//! operator's).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};

use crate::{demul, mono_imports};

const PLAYERS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Fire1,
    Fire2,
    Coin,
    Setting,
    Left,
    Right,
    Ticket,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("fire1", Native::Fire1),
    ("fire2", Native::Fire2),
    ("coin", Native::Coin),
    ("setting", Native::Setting),
    ("left", Native::Left),
    ("right", Native::Right),
    ("ticket", Native::Ticket),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "fire1"),
    ("b2", "fire2"),
    ("b3", "fire1+fire2"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "setting"),
    ("left", "left"),
    ("right", "right"),
    ("service", "ticket"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn held(player: usize, native: Native) -> bool {
    let mut on = false;
    map().for_each_pressed(&input(player), |n| on |= n == native);
    on
}

/// The coins each player inserted, not credited yet.
static COINS: [AtomicI32; PLAYERS] = [const { AtomicI32::new(0) }; PLAYERS];

/// Counts the coin presses.
fn coin_thread() {
    let mut was = [false; PLAYERS];
    loop {
        std::thread::sleep(Duration::from_millis(8));
        for (player, was) in was.iter_mut().enumerate() {
            let now = held(player, Native::Coin);
            if now && !*was {
                COINS[player].fetch_add(1, Ordering::Relaxed);
            }
            *was = now;
        }
    }
}

/// The ADC reading of `player`'s gun (`y`: the vertical one).
fn adc(player: usize, y: bool) -> u16 {
    let (px, py) = demul::gun_of_player_in_picture(player).unwrap_or((0.5, 0.5));
    let v = if y { 1.0 - py } else { px };
    (1.0 + 1022.0 * v.clamp(0.0, 1.0)).round() as u16
}

type P = *mut c_void;

/// The object `GetInstance` returns: the functions ignore it.
static INSTANCE: [u8; 16] = [0; 16];

extern "C" fn get_instance() -> P {
    INSTANCE.as_ptr() as P
}

extern "C" fn init(_dll: P, _param: i32) -> i32 {
    log!("gameport: init");
    1
}

/// Outputs and the other setters: any arguments (cdecl: the caller pops them).
extern "C" fn ignored() {}

/// The other getters: false, 0.
extern "C" fn zero() -> i32 {
    0
}

macro_rules! per_player {
    ($($p:literal),*) => {
        /// `name`'s function of player `p` (0-based).
        fn player_function(kind: &str, p: usize) -> Option<usize> {
            match (kind, p) {
                $(
                    ("start", $p) => { extern "C" fn f(_: P) -> i32 { held($p, Native::Start) as i32 } Some(f as *const () as usize) }
                    ("fire1", $p) => { extern "C" fn f(_: P) -> i32 { held($p, Native::Fire1) as i32 } Some(f as *const () as usize) }
                    ("fire2", $p) => { extern "C" fn f(_: P) -> i32 { held($p, Native::Fire2) as i32 } Some(f as *const () as usize) }
                    ("coins", $p) => { extern "C" fn f(_: P) -> i32 { COINS[$p].load(Ordering::Relaxed) } Some(f as *const () as usize) }
                    ("subcoins", $p) => {
                        extern "C" fn f(_: P, count: i32) {
                            let _ = COINS[$p].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some((c - count).max(0)));
                        }
                        Some(f as *const () as usize)
                    }
                    ("x", $p) => { extern "C" fn f(_: P) -> u16 { adc($p, false) } Some(f as *const () as usize) }
                    ("y", $p) => { extern "C" fn f(_: P) -> u16 { adc($p, true) } Some(f as *const () as usize) }
                )*
                _ => None,
            }
        }
    };
}

per_player!(0, 1, 2, 3);

extern "C" fn setting(_: P) -> i32 {
    held(0, Native::Setting) as i32
}

extern "C" fn left(_: P) -> i32 {
    held(0, Native::Left) as i32
}

extern "C" fn right(_: P) -> i32 {
    held(0, Native::Right) as i32
}

extern "C" fn ticket(_: P) -> i32 {
    held(0, Native::Ticket) as i32
}

/// The player (0-based) of names like `GetP2Frie1`, `GetBufferCoin3p`, `GetWadc1pX`.
fn player_of(name: &str) -> Option<usize> {
    let digit = |i: usize| name.as_bytes().get(i).filter(|c| c.is_ascii_digit()).map(|c| (c - b'1') as usize);
    let n = if let Some(rest) = name.strip_prefix("GetP") { digit(name.len() - rest.len()) } else { name.find(|c: char| c.is_ascii_digit()).and_then(digit) };
    n.filter(|p| *p < PLAYERS)
}

/// The board's functions.
fn resolve(name: &str, module: &str) -> Option<usize> {
    if module != "gameport.dll" {
        return None;
    }
    let f = |f: usize| Some(f);
    match name {
        "GetInstance" => f(get_instance as *const () as usize),
        "init" => f(init as *const () as usize),
        "GetSettingKey" => f(setting as *const () as usize),
        "GetLeftKey" => f(left as *const () as usize),
        "GetRightKey" => f(right as *const () as usize),
        "GetTicketKey" => f(ticket as *const () as usize),
        _ => {
            let player = player_of(name);
            let kind = if name.ends_with("StartKey") {
                Some("start")
            } else if name.ends_with("Frie1") {
                Some("fire1")
            } else if name.ends_with("Frie2") {
                Some("fire2")
            } else if name.starts_with("GetBufferCoin") {
                Some("coins")
            } else if name.starts_with("SubBufferCoin") {
                Some("subcoins")
            } else if name.contains("Wadc") && name.ends_with('X') {
                Some("x")
            } else if name.contains("Wadc") && name.ends_with('Y') {
                Some("y")
            } else {
                None
            };
            match (kind, player) {
                (Some(kind), Some(p)) => player_function(kind, p),
                // the outputs, the board's state (no error, no empty ticket hopper, water fine)
                _ if name.starts_with("Set") || name.starts_with("Sub") || name.starts_with("Clear") => f(ignored as *const () as usize),
                _ if name.starts_with("Get") => f(zero as *const () as usize),
                // the rest (Fire Hero's camera: OpenCam, ShowCamWnd...)
                _ => f(ignored as *const () as usize),
            }
        }
    }
}

pub fn init_board() {
    mono_imports::hook(resolve);
    std::thread::spawn(coin_thread);
    log!("gameport: the gameport board's functions");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players() {
        assert_eq!(player_of("GetP2Frie1"), Some(1));
        assert_eq!(player_of("GetBufferCoin3p"), Some(2));
        assert_eq!(player_of("GetWadc1pX"), Some(0));
        assert_eq!(player_of("GetP4WadcY"), Some(3));
        assert_eq!(player_of("GetSettingKey"), None);
    }
}
