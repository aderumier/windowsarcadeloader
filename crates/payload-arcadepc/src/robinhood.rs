//! Robin Hood (`data\game.exe`, Direct3D 9, 32-bit): `WAL_ARCADEPC_BOARD=robinhood`, x86 build.
//!
//! - **Working directory.** The game reads `.\game.ini`, `log\PR*.CFG` and `data\game.bin`
//!   from the dump's root (the cabinet's `start.bat` runs `data\game.exe` from there); the
//!   launcher starts it in `data`: the payload goes up one folder before the game's code runs.
//! - **Board.** The first COM port that opens (COM1 to COM4, 9600 bauds) is the cabinet's board.
//!   The game's thread sends 7-byte packets `cmd, its time (4 bytes), arg, sum` (the sum of the
//!   first six bytes) and reads the board's packets until the echo of its command. The board's
//!   packets: one byte for `00`, `FF`, `94`-`98`, `#`, `$`, `)`, `0`, else 7 bytes `cmd, d1-d5,
//!   sum`. The game polls with `%` once per frame; the board answers one packet per held input
//!   (the cabinet's switches, as the I/O check names them: `80` advance, `81` spare, `84`
//!   select, `C` admin (the test menu), `83` start, `D` free credit), one `@` per coin, then
//!   `%`: the packets seen until then are the
//!   frame's inputs (`0x48f8c0`). The game's other commands: `29` the coin setting, `94` the I/O
//!   mode, `95` a lamp, `96` tickets to pay, `30` the dispenser's state (tickets left: `Q` / `P`
//!   with the count's low / high byte; `B` a vending error). Here the tickets are paid at once:
//!   `30` gets `Q 0`, `P 0`.
//! - **Crossbow.** A lightgun game: a DirectInput joystick (the common `WAL_DINPUT_PADS`, `lx`
//!   the gun's X position): its X axis (range -1000..1000) the aim across the screen, its
//!   buttons `jtrigb` (0) the trigger and `jstart` (1) start. The game also reads the PC keyboard (`GetKeyboardState`: the mouse's left button
//!   and space shoot, Enter starts, `T` / Escape test...): hidden (`WAL_KEYBOARD_STATE_DISABLE`).
//! - **Dongle.** A HASP HL, its API linked in the executable: the profile's patches log in at
//!   once and its memory reads `GWRH` (else the boot stops on error 12).
//! - **Free play.** `coinfree` of the operator settings (`log\PR*.CFG`), loaded at `0x48f1d0`:
//!   the common `WAL_FREEPLAY` replaces the loaded value (the dump's: on).
//!
//! Natives: `left right down start coin service test`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};

const PORT: &str = "COM1";
/// The load of `coinfree` (`mov eax, [eax+8]; mov ecx, [eax]`), relative to the executable:
/// replaced with `mov ecx, value`.
const FREE_PLAY_LOAD: usize = 0x8f1e5;

const POLL: u8 = b'%';
const COIN: u8 = b'@';
const TICKETS_STATE: u8 = b'0';
const TICKETS_LOW: u8 = b'Q';
const TICKETS_HIGH: u8 = b'P';

/// The board's packets of one byte.
fn short(cmd: u8) -> bool {
    matches!(cmd, 0x00 | 0xFF | 0x94..=0x98 | b'#' | b'$' | b')' | b'0')
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A held input's packet.
    Held(u8),
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("left", Native::Held(0x80)),
    ("right", Native::Held(0x81)),
    ("down", Native::Held(0x84)),
    ("start", Native::Held(0x83)),
    ("test", Native::Held(b'C')),
    ("service", Native::Held(b'D')),
    ("coin", Native::Coin),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("left", "left"),
    ("right", "right"),
    ("down", "down"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The coin's state at the last poll (one `@` per press).
static COIN_HELD: AtomicBool = AtomicBool::new(false);
static POLLS: AtomicU64 = AtomicU64::new(0);

/// A 7-byte packet of the board.
fn packet(cmd: u8, d1: u8) -> [u8; 7] {
    let mut p = [cmd, d1, 0, 0, 0, 0, 0];
    p[6] = p[..6].iter().fold(0u8, |s, &b| s.wrapping_add(b));
    p
}

/// The echo that ends the board's answer to `cmd`.
fn echo(cmd: u8) -> Vec<u8> {
    if short(cmd) { vec![cmd] } else { packet(cmd, 0).to_vec() }
}

/// The frame's inputs: one packet per held input, `@` for a new coin, then `%`.
fn poll(held: &[u8], coin: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for &cmd in held {
        out.extend_from_slice(&packet(cmd, 0));
    }
    if coin {
        out.extend_from_slice(&packet(COIN, 1));
    }
    out.extend_from_slice(&packet(POLL, 0));
    out
}

fn handler(p: &[u8]) -> Vec<u8> {
    let Some(&cmd) = p.first() else { return Vec::new() };
    match cmd {
        POLL => {
            let mut held = Vec::new();
            let mut coin = false;
            map().for_each_pressed(&input(0), |n| match n {
                Native::Held(c) if !held.contains(&c) => held.push(c),
                Native::Held(_) => {}
                Native::Coin => coin = true,
            });
            let new_coin = coin && !COIN_HELD.swap(coin, Ordering::Relaxed);
            if !coin {
                COIN_HELD.store(false, Ordering::Relaxed);
            }
            if POLLS.fetch_add(1, Ordering::Relaxed) == 0 {
                log!("robinhood: first poll of the board");
            }
            poll(&held, new_coin)
        }
        TICKETS_STATE => {
            // no ticket left to pay
            let mut out = packet(TICKETS_LOW, 0).to_vec();
            out.extend_from_slice(&packet(TICKETS_HIGH, 0));
            out.extend_from_slice(&echo(cmd));
            out
        }
        0x96 => {
            log!("robinhood: {} ticket(s) paid", p.get(5).copied().unwrap_or(0));
            echo(cmd)
        }
        _ => echo(cmd),
    }
}

pub fn init() {
    let ok = std::env::set_current_dir("..").is_ok();
    log!("robinhood: working directory {:?}", if ok { std::env::current_dir().ok() } else { None });
    if let Some(on) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
        wal_payload_common::patches::apply_list(&format!("{FREE_PLAY_LOAD:#x}:B9{:02X}000000", on as u8));
        log!("robinhood: free play {}", if on { "on" } else { "off" });
    }
    serial::install(PORT, handler);
    log!("robinhood: board on {PORT}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets() {
        assert_eq!(packet(POLL, 0), [0x25, 0, 0, 0, 0, 0, 0x25]);
        assert_eq!(packet(COIN, 1), [0x40, 1, 0, 0, 0, 0, 0x41]);
        assert_eq!(echo(b')'), vec![0x29]);
        assert_eq!(echo(POLL).len(), 7);
        let p = poll(&[b'C'], true);
        assert_eq!(p.len(), 21);
        assert_eq!(&p[14..], &packet(POLL, 0));
    }
}
