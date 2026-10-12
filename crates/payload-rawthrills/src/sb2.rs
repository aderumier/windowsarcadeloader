//! The Fast and the Furious Super Bikes 2 (2010 build), `WAL_RAWTHRILLS_GAME=superbikes2`.
//!
//! The SuperCars engine (`SetControl`, board codes `0x10000 + n`), but the dump's input
//! functions (held `0x4bba00`, pressed once `0x4bba90`) read every code as a keyboard key:
//! `and ecx, 0xff; jmp` in place of `test ecx, 0x10000; je`. So a board code reads the keyboard
//! state byte `code & 0xff` of the game's table (`0x8901438`, bit 0 held, bit 1 pressed once,
//! cleared by the read): START `0x00`, VIEW `0x01`, TUNES `0x04`, SHIFT `0x05`, coin 1 / 2
//! `0x09` / `0x0a`, TEST `0x0b`, SERVICE `0x0c`, bill `0x0d`, volume up / down `0x0e` /
//! `0x0f`, the digital brake `0x10`, the keypad `0x80..0x8b`. The game's DirectInput keyboard
//! is not created (its input init `0x4126bf` removed): a thread of ours writes those bytes
//! from the virtual sticks. The developer key Down (`0xd0`) shifts down.
//!
//! The analog values (handlebar, throttle, brake: `0x10020..0x10022`) are read from the
//! game's analog table (`0x8901548`, a dword per code `& 0xff`, 0..255), written every poll:
//! the game turns them into its floats through its calibration (test menu).
//!
//! Build fixes: the back buffer format kept (`0x4b7bd2`: R5G6B5 crashed), the developer key
//! Space (ends the race) made Escape, no error box on close, no Watson, no Escape box, the
//! files' base path left empty (`0x4aa8a0`).

use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, button};

use crate::common::{KEYPAD_NONE, Site, keypad_key, matches, pedal, write};

/// The game's keyboard state table (one byte per key code).
const KEYS: usize = 0x8901438;
/// The game's analog table (a dword per code `& 0xff`).
const ANALOG: usize = 0x8901548;

/// Checked sites and their replacement.
const PATCHES: &[(Site, &[u8])] = &[
    // the input init (DirectInput keyboard and mouse, the board): removed
    ((0x4126bf, &[0xe8, 0x5c, 0x9b, 0x0a, 0x00]), &[0x90; 5]),
    // the files' base path: `xor eax, eax; ret`
    ((0x4aa8a0, &[0x8b, 0x44, 0x24, 0x04, 0x85, 0xc0, 0x57]), &[0x33, 0xc0, 0xc3]),
    // the back buffer format kept (R5G6B5 crashed)
    ((0x4b7bd2, &[0x74, 0x0f]), &[0xeb, 0x0f]),
    // the developer key Space (ends the race) made Escape
    ((0x471a81, &[0x6a, 0x39]), &[0x6a, 0x01]),
    // no error box on close, no Watson, no Escape box
    ((0x411c02, &[0x74, 0x12]), &[0xeb, 0x12]),
    ((0x529e62, &[0x75, 0x1d]), &[0x90, 0x90]),
    ((0x4714da, &[0xe8, 0x81, 0xcd, 0xff, 0xff]), &[0x90; 5]),
];

/// Board input: its keyboard state byte.
const NATIVES: &[(&str, u8)] = &[
    ("start", 0x00),
    ("view", 0x01),
    ("tunes", 0x04),
    ("shift", 0x05),
    ("shiftdown", 0xd0),
    ("coin1", 0x09),
    ("coin2", 0x0a),
    ("test", 0x0b),
    ("service", 0x0c),
    ("bill", 0x0d),
    ("volup", 0x0e),
    ("voldown", 0x0f),
    ("brake", 0x10),
];

/// Virtual stick -> board, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "view"),
    ("b2", "tunes"),
    ("b3", "brake"),
    ("b4", "bill"),
    ("b5", "shiftdown"),
    ("b6", "shift"),
    ("up", "volup"),
    ("down", "voldown"),
];

/// Keypad bytes by keypad key index (1-9, *, 0, #).
const KEYPAD: [u8; 12] = [0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x80, 0x8b];
/// Brake pedal travel pressing the digital brake.
const BRAKE_SWITCH: i16 = 32767 / 3;
const POLL: Duration = Duration::from_millis(4);

static MAP: OnceLock<ButtonMap<u8>> = OnceLock::new();

/// Patches Super Bikes 2 when `WAL_RAWTHRILLS_GAME=superbikes2`.
pub(crate) fn init(name: &str) -> bool {
    if !name.eq_ignore_ascii_case("superbikes2") {
        return false;
    }
    if let Some(((site, _), _)) = PATCHES.iter().find(|(s, _)| !matches(*s)) {
        log!("superbikes2: unexpected code at {site:#x}, not this build, left alone");
        return true;
    }
    for ((at, _), code) in PATCHES {
        write(*at, code);
    }
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    let keypad = std::env::var("WAL_RAWTHRILLS_KEYPAD").map_or(true, |v| v != "0");
    std::thread::spawn(move || run(keypad));
    log!("superbikes2: board inputs in the game's keyboard table, keyboard controls off");
    true
}

/// Sets a key's state: held (bit 0), a press latched (bit 1) on the press edge.
fn set_key(key: u8, down: bool, was_down: bool) {
    let at = (KEYS + key as usize) as *mut u8;
    unsafe {
        let v = at.read_volatile();
        let v = if down { v | 1 | if was_down { 0 } else { 2 } } else { v & !1 };
        at.write_volatile(v);
    }
}

fn run(keypad: bool) {
    let map = MAP.get().expect("map set by init");
    let mut held = [false; 256];
    loop {
        let stick = wal_payload_common::input(0);
        let mut down = [false; 256];
        map.for_each_pressed(&stick, |key| down[key as usize] = true);
        if wal_payload_common::input(1).pressed(button::COIN) {
            down[0x0a] = true;
        }
        if stick.axis(Axis::Brake) > BRAKE_SWITCH {
            down[0x10] = true;
        }
        let key = if keypad { keypad_key() } else { KEYPAD_NONE };
        if key != KEYPAD_NONE {
            down[KEYPAD[key as usize] as usize] = true;
        }
        for k in 0..256 {
            if down[k] || held[k] {
                set_key(k as u8, down[k], held[k]);
            }
        }
        held = down;
        for (code, value) in [(0x20, stick.axis_u8(Axis::LeftX)), (0x21, pedal(&stick, Axis::Accel)), (0x22, pedal(&stick, Axis::Brake))] {
            unsafe { ((ANALOG + code * 4) as *mut u32).write_volatile(value as u32) };
        }
        std::thread::sleep(POLL);
    }
}
