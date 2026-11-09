//! Burning Striker (Eolith, 2003; `ewcs.exe`, DirectDraw / Direct3D 7, 32-bit):
//! `WAL_ARCADEPC_BOARD=eolith`, x86 build.
//!
//! - **Protection.** The game (`0x40a5a0`) creates the shared memory `_buring_shared_mem` and
//!   runs `vault.exe` through `TWIMNET.DLL`, which copies 15 bytes of the cabinet's BIOS
//!   (F000:E05E, read with the absent `AccessHW.dll`) to its offset 8 and starts the game again;
//!   the second instance sets the status (offset 4) to 6 and goes on. Those bytes are the key of
//!   `3ddata.pak`'s directory (IDEA). Here the check returns 1 at once (the profile's patch) and
//!   the shared memory is ours (`0x565974` points at it): status 6, the key. The key is not in
//!   our sources: the dump's `eolith.key` (next to `ewcs.exe`, its bytes as they are; a final
//!   newline ignored).
//! - **Board.** COM1 (9600 bauds): the game's thread reads it every 33 ms, frames
//!   `00 b1 b2 b3 b4 50`, active low. `b1` / `b2` player 1 / 2: bit 7 up, 6 down, 5 left,
//!   4 right, 3 / 2 / 1 buttons A / B / C. `b3`: bit 6 player 1 start, 2 player 2 start, 5 / 1
//!   the coins (counted when held two frames, released two frames), 4 service (a credit), 0 test.
//!   Bit 0 of `b1` / `b2` and bit 7 of `b3` are always released, so a data byte is never `00`
//!   (frame start) nor `50` (frame end).
//! - **DIP switches.** `b4 >> 1` (bit 0 of `b4` always set, same reason), kept at `0x56536c`
//!   and read once at boot (`0x479e20` through `0x403820`), right after the port is opened,
//!   before the board's first frame: the getter returns ours (patched `mov al, dip; ret`). Bits
//!   0-1 the level (0: the dump's level 2), 2-3 the coins per credit (1: one), 4 the play time
//!   (0: 7), 5-6 the mode: 1 free play (attract with sound), 0 coins (attract silent, the dump's
//!   `eoconfig.ini`). The common `WAL_FREEPLAY` picks the mode.
//!
//! Natives: `up down left right a b c start coin` (each player), `service test`.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};

const PORT: &str = "COM1";
/// The game's pointer to the shared memory of the protection.
const SHARED_POINTER: usize = 0x565974;
const STATUS_OK: u32 = 6;
/// The dump's key file, in the game's directory.
const KEY_FILE: &str = "eolith.key";
/// The game's DIP switch getter (`mov al, [0x56536c]; ret`), relative to the executable.
const DIP_GETTER: usize = 0x3820;
/// Level 2, one coin per credit, play time 7.
const DIP_BASE: u8 = 0x04;
const DIP_FREE_PLAY: u8 = 0x20;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A bit of the player's byte (`b1` / `b2`).
    Player(u8),
    Start,
    Coin,
    /// A bit of `b3`.
    Operator(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("up", Native::Player(7)),
    ("down", Native::Player(6)),
    ("left", Native::Player(5)),
    ("right", Native::Player(4)),
    ("a", Native::Player(3)),
    ("b", Native::Player(2)),
    ("c", Native::Player(1)),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("service", Native::Operator(4)),
    ("test", Native::Operator(0)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "a"),
    ("b2", "b"),
    ("b3", "c"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// A coin press stays on the wire this long: the game counts it after two frames.
const COIN_HOLD: Duration = Duration::from_millis(150);

/// Each player's last coin press.
static COINS: Mutex<[Option<Instant>; 2]> = Mutex::new([None; 2]);
static FRAMES: AtomicU64 = AtomicU64::new(0);
/// The DIP switches (`b4 >> 1`).
static DIP: AtomicU8 = AtomicU8::new(DIP_BASE);

/// The frame of the pressed inputs (bits set: pressed).
fn frame(players: [u8; 2], operator: u8, dip: u8) -> [u8; 6] {
    [0x00, !players[0] | 1, !players[1] | 1, !operator | 0x80, dip << 1 | 1, 0x50]
}

/// The board's frame each time the game reads the port.
fn idle() -> Vec<u8> {
    let mut players = [0u8; 2];
    let mut operator = 0u8;
    let mut coins = COINS.lock().unwrap();
    for (player, bits) in players.iter_mut().enumerate() {
        let stick = input(player);
        map().for_each_pressed(&stick, |n| match n {
            Native::Player(bit) => *bits |= 1 << bit,
            Native::Start => operator |= if player == 0 { 1 << 6 } else { 1 << 2 },
            Native::Coin => coins[player] = Some(Instant::now()),
            Native::Operator(bit) if player == 0 => operator |= 1 << bit,
            Native::Operator(_) => {}
        });
        if coins[player].is_some_and(|t| t.elapsed() < COIN_HOLD) {
            operator |= if player == 0 { 1 << 5 } else { 1 << 1 };
        }
    }
    if FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("eolith: first frame to the game");
    }
    frame(players, operator, DIP.load(Ordering::Relaxed)).to_vec()
}

/// The game writes nothing to the board.
fn handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

/// The DIP switches for the common free play option (`None`: the cabinet's coins mode).
fn dip(free_play: Option<bool>) -> u8 {
    DIP_BASE | if free_play == Some(true) { DIP_FREE_PLAY } else { 0 }
}

/// The protection's shared memory, the key at offset 8.
fn shared_memory(key: &[u8]) -> &'static mut [u8; 24] {
    let shared = Box::leak(Box::new([0u8; 24]));
    shared[4..8].copy_from_slice(&STATUS_OK.to_le_bytes());
    let n = key.len().min(15);
    shared[8..8 + n].copy_from_slice(&key[..n]);
    shared
}

/// The key file's bytes, without a final newline.
fn read_key() -> Option<Vec<u8>> {
    let mut key = std::fs::read(KEY_FILE).ok()?;
    while key.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
        key.pop();
    }
    (!key.is_empty()).then_some(key)
}

pub fn init() {
    match read_key() {
        Some(key) => {
            let shared = shared_memory(&key);
            unsafe { *(SHARED_POINTER as *mut usize) = shared.as_ptr() as usize };
        }
        None => log!("eolith: no {KEY_FILE} in the game directory, 3ddata.pak cannot be read"),
    }
    let dip = dip(wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY"));
    DIP.store(dip, Ordering::Relaxed);
    wal_payload_common::patches::apply_list(&format!("{DIP_GETTER:#x}:B0{dip:02X}C3"));
    serial::install_streaming(PORT, handler, idle);
    log!("eolith: board on {PORT}, DIP switches {dip:#04x}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        // nothing pressed
        assert_eq!(frame([0, 0], 0, 0), [0x00, 0xFF, 0xFF, 0xFF, 0x01, 0x50]);
        // everything pressed, any DIP: still no 00 / 50 in the data
        for dip in 0..=0x7F {
            let f = frame([0xFE, 0xFE], 0x7F, dip);
            assert!(f[1..5].iter().all(|&b| b != 0x00 && b != 0x50));
        }
        assert_eq!(frame([0, 0], 0, 0x24)[4], 0x49);
    }

    #[test]
    fn dips() {
        assert_eq!((dip(Some(true)), dip(Some(false)), dip(None)), (0x24, 0x04, 0x04));
    }

    #[test]
    fn shared() {
        let s = shared_memory(b"eo-key");
        assert_eq!(&s[4..8], &[6, 0, 0, 0]);
        assert_eq!(&s[8..14], b"eo-key");
        assert_eq!(s[23], 0);
    }
}
