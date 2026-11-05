//! Mighty Moto (UNIS's motorbike cabinet, Unity 2.x, 32-bit, 2009): `WAL_UNIS_BOARD=mightymoto`.
//!
//! The cabinet's controls are a keyboard (its I/O board a key encoder): the game's scripts read
//! Unity's `Input` keys and its `Horizontal` axis (`headScript`, `myCar`, `houtai`: the operator
//! menu), which Unity 2.x takes from a DirectInput keyboard. The payload's fake DirectInput
//! (`WAL_DINPUT_DISABLE=1`, `payload-common`'s `dinput.rs`) has a system keyboard whose keys
//! come from player 1's virtual stick:
//!
//! The game's window gets the same keys as key messages (the payload's own, through the input
//! filter), and its key state polls see them held: Unity 2.x reads the keyboard from its window
//! messages.
//!
//! - `1` start, `2` the item (a power-up), `3` a coin, `4` the operator menu (`houtai`: `j` /
//!   `l` move, `d` enters), `d` / up arrow the gas, `u` the brake, `s` back, space the
//!   handbrake, the left / right arrows the handlebar (the stick's x past a third of its
//!   travel).
//!
//! Natives: `start item coin test gas brake back handbrake left right menuleft menuright
//! menuenter`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{dinput, gun, input, input_filter, log};
use wal_protocol::Axis;

/// DirectInput key codes.
const DIK_1: u8 = 0x02;
const DIK_2: u8 = 0x03;
const DIK_3: u8 = 0x04;
const DIK_4: u8 = 0x05;
const DIK_S: u8 = 0x1F;
const DIK_D: u8 = 0x20;
const DIK_U: u8 = 0x16;
const DIK_J: u8 = 0x24;
const DIK_L: u8 = 0x26;
const DIK_SPACE: u8 = 0x39;
const DIK_LEFT: u8 = 0xCB;
const DIK_RIGHT: u8 = 0xCD;

const NATIVES: &[(&str, u8)] = &[
    ("start", DIK_1),
    ("item", DIK_2),
    ("coin", DIK_3),
    ("test", DIK_4),
    ("gas", DIK_D),
    ("brake", DIK_U),
    ("back", DIK_S),
    ("handbrake", DIK_SPACE),
    ("left", DIK_LEFT),
    ("right", DIK_RIGHT),
    ("menuleft", DIK_L),
    ("menuright", DIK_J),
    ("menuenter", DIK_D),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("b1", "gas"),
    ("b2", "brake"),
    ("b3", "item"),
    ("b4", "back"),
    ("coin", "coin"),
    ("left", "left"),
    ("right", "right"),
    // the operator menu: test opens it, service enters, up / down move
    ("test", "test"),
    ("service", "menuenter"),
    ("up", "menuleft"),
    ("down", "menuright"),
];

fn map() -> &'static ButtonMap<u8> {
    static MAP: OnceLock<ButtonMap<u8>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keyboard's keys: player 1's, the stick's x as the arrows, the gas pedal's travel as the
/// gas key.
fn keyboard(keys: &mut [u8; 256]) {
    let stick = input(0);
    map().for_each_pressed(&stick, |dik| keys[dik as usize] = 0x80);
    let x = stick.axis(Axis::LeftX);
    if x < -11000 {
        keys[DIK_LEFT as usize] = 0x80;
    } else if x > 11000 {
        keys[DIK_RIGHT as usize] = 0x80;
    }
    if stick.axis(Axis::Accel) > 8000 {
        keys[DIK_D as usize] = 0x80;
    }
    if stick.axis(Axis::Brake) > 8000 {
        keys[DIK_U as usize] = 0x80;
    }
}

/// A DirectInput key's virtual key.
fn virtual_key(dik: u8) -> Option<u16> {
    Some(match dik {
        DIK_1..=DIK_4 => b'1' as u16 + (dik - DIK_1) as u16,
        DIK_S => b'S' as u16,
        DIK_D => b'D' as u16,
        DIK_U => b'U' as u16,
        DIK_J => b'J' as u16,
        DIK_L => b'L' as u16,
        DIK_SPACE => 0x20,
        DIK_LEFT => 0x25,
        DIK_RIGHT => 0x27,
        _ => return None,
    })
}

/// The virtual keys held (a bit each, 256 keys).
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn bit(vk: u16) -> (usize, u64) {
    (vk as usize / 64 % 4, 1 << (vk % 64))
}

/// `input_filter`'s key state: the keys posted held.
pub fn key_held(vk: i32) -> bool {
    let (word, bit) = bit((vk & 0xFF) as u16);
    HELD[word].load(Ordering::Relaxed) & bit != 0
}

/// Posts the keys' changes to the game's window every 10 ms.
fn post_keys() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let window = gun::find_window();
            if window.is_null() {
                continue;
            }
            let mut keys = [0u8; 256];
            keyboard(&mut keys);
            for dik in [DIK_1, DIK_2, DIK_3, DIK_4, DIK_S, DIK_D, DIK_U, DIK_J, DIK_L, DIK_SPACE, DIK_LEFT, DIK_RIGHT] {
                let Some(vk) = virtual_key(dik) else { continue };
                let (was, is) = (key_held(vk as i32), keys[dik as usize] != 0);
                if was == is {
                    continue;
                }
                // held first, so the game's key state polling agrees with the message
                let (word, bit) = bit(vk);
                if is {
                    HELD[word].fetch_or(bit, Ordering::Relaxed);
                }
                input_filter::post_key(window, vk, is);
                if !is {
                    HELD[word].fetch_and(!bit, Ordering::Relaxed);
                }
            }
        }
    });
}

pub fn init() {
    dinput::set_keyboard(keyboard);
    post_keys();
    log!("mightymoto: keyboard keys from player 1's stick (DirectInput and window messages)");
}
