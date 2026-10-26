//! The "JX" I/O board of JxSoftWare's cabinets (Mech Dinosaur, Wisdom Zombies; Unity 2017.2,
//! 64-bit): `WAL_ARCADEPC_BOARD=jxboard`.
//!
//! The game's `PortManager` opens it on COM1 (`Game.json`: 115200 bauds) with Mono's
//! `SerialPort` (its kernel32 functions resolved to `payload-common`'s `serial` through
//! `mono_imports.rs`). Frames both ways `3A len data[len] crc_hi crc_lo 3B`, `data[0]` the
//! command, the CRC (the game's `CrcParse`: CRC-16/MODBUS's table, its bytes swapped at each
//! step, starting at 0xFFFF) over `3A`, `len` and the data. The board sends command 1 (40
//! bytes) all the time (the game shows a communication fault after 300 s without it): 24 key
//! bits (0..3 the players' triggers, 4..7 their start keys, 19 the water level fine, 20..23
//! the operator's menu, OK, up and down; Wisdom Zombies swaps up and down: its profile's
//! `native_map`), the guns' 8 axes (x, y per player, big endian, 0..4094: 2047 the screen's
//! middle, y up), the tickets left to pay (0) and the coins inserted since the last frame. The
//! PC's frames (lamps, motors, tickets to pay) are ignored.
//!
//! Natives: `trigger start coin` (each player), `menu ok up down` (the operator's).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};

use crate::{demul, mono_imports};

const PORT: &str = "COM1";
const PLAYERS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Start,
    Coin,
    /// An operator key's bit (20..23).
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("menu", Native::Key(20)),
    ("ok", Native::Key(21)),
    ("up", Native::Key(22)),
    ("down", Native::Key(23)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "menu"),
    ("service", "ok"),
    ("b2", "ok"),
    ("up", "up"),
    ("down", "down"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The game's `CrcParse.CrcVal`.
fn crc(data: &[u8]) -> [u8; 2] {
    static TABLE: OnceLock<[u16; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u16; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u16;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xA001 } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut v: u32 = 0xFFFF;
    for &b in data {
        let i = ((v >> 8) ^ b as u32) & 0xFF;
        let t = table[i as usize] as u32;
        v = (((v ^ t) & 0xFF) << 8) | (t >> 8);
    }
    [(v >> 8) as u8, v as u8]
}

/// A board frame: `3A len data crc 3B`.
fn frame(data: &[u8]) -> Vec<u8> {
    let mut f = vec![0x3A, data.len() as u8];
    f.extend_from_slice(data);
    let c = crc(&f);
    f.extend(c);
    f.push(0x3B);
    f
}

/// The coin presses not reported yet, per player; the last report's time.
struct State {
    coins_held: [bool; PLAYERS],
    last: Instant,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// The board's command 1 when the game reads the port (every 8 ms at most).
fn idle() -> Vec<u8> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(|| State { coins_held: [false; PLAYERS], last: Instant::now() - Duration::from_secs(1) });
    if state.last.elapsed() < Duration::from_millis(8) {
        return Vec::new();
    }
    state.last = Instant::now();
    let mut data = vec![0u8; 40];
    data[0] = 1;
    // the water level fine
    let mut keys: u32 = 1 << 19;
    for player in 0..PLAYERS {
        let stick = input(player);
        let mut coin = false;
        map().for_each_pressed(&stick, |n| match n {
            Native::Trigger => keys |= 1 << player,
            Native::Start => keys |= 1 << (4 + player),
            Native::Coin => coin = true,
            Native::Key(bit) if player == 0 => keys |= 1 << bit,
            Native::Key(_) => {}
        });
        if coin && !state.coins_held[player] {
            data[36 + player] = 1;
        }
        state.coins_held[player] = coin;
        let (x, y) = demul::gun_of_player_in_picture(player).unwrap_or((0.5, 0.5));
        let ax = (x.clamp(0.0, 1.0) * 4094.0).round() as u16;
        let ay = ((1.0 - y.clamp(0.0, 1.0)) * 4094.0).round() as u16;
        data[4 + player * 4..8 + player * 4].copy_from_slice(&[ax.to_be_bytes(), ay.to_be_bytes()].concat());
    }
    data[1..4].copy_from_slice(&keys.to_le_bytes()[..3]);
    if FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("jxboard: first frame to the game");
    }
    frame(&data)
}

/// The PC's frames: outputs, nothing to answer.
fn handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module == "kernel32.dll" { serial::replacement(name) } else { None }
}

pub fn init() {
    serial::install_streaming_in(PORT, handler, idle, &[]);
    mono_imports::hook(resolve);
    log!("jxboard: JX board on {PORT}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let f = frame(&[1, 2, 3]);
        assert_eq!((f[0], f[1], f[f.len() - 1]), (0x3A, 3, 0x3B));
    }
}
