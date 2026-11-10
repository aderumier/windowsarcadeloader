//! Savage Quest (Immersia, 1999, DOS / DOS4GW, Glide): `WAL_ARCADEPC_BOARD=savagequest`, x86
//! build.
//!
//! The dump runs the DOS game (`PSAVAGE.EXE`, "SQ v0.92 May 13 1999") in the community
//! `winsavage.exe`, a Windows loader that maps it flat, calls the dump's `Glide2x.dll` and reads
//! one XInput pad (`LoadLibraryA("xinput1_3.dll")`: the payload). Each frame it takes the first
//! connected pad into the game's GCI joystick (the cabinet's Gandalf board's: the object at game
//! +0x4fe104, its joystick at +0x3c): +0x79c = LY / 32767 (the game's x, +right), +0x7a4 =
//! LX / 32767 (its y, +back), +0x7ac bit 0 A,
//! bit 1 X, bit 2 Y, bit 3 START. Its window's keys (`WM_KEYDOWN`) become DOS scancodes in the
//! game's keyboard array: the real keyboard is filtered (the executable's `PeekMessageW`).
//!
//! Pad: the left stick (or the directions, at full deflection), b1 bite (A), b2 roar (X), b3 run
//! (Y: the stick's top button), start (START).

use std::sync::OnceLock;

use wal_payload_common::input;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::{Axis, button};

use crate::pads::Gamepad;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Bite,
    Roar,
    Run,
    Start,
}

const NATIVES: &[(&str, Native)] = &[("bite", Native::Bite), ("roar", Native::Roar), ("run", Native::Run), ("start", Native::Start)];

const DEFAULTS: &[(&str, &str)] = &[("b1", "bite"), ("b2", "roar"), ("b3", "run"), ("start", "start")];

const XINPUT_GAMEPAD_START: u16 = 0x0010;
const XINPUT_GAMEPAD_A: u16 = 0x1000;
const XINPUT_GAMEPAD_X: u16 = 0x4000;
const XINPUT_GAMEPAD_Y: u16 = 0x8000;

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// One stick axis: the analog one, else the directions at full deflection.
fn axis(analog: i16, minus: bool, plus: bool) -> i16 {
    match (minus, plus) {
        (true, false) if analog == 0 => -i16::MAX,
        (false, true) if analog == 0 => i16::MAX,
        _ => analog,
    }
}

/// Player 1's pad, as `winsavage.exe` reads it (pads.rs' `XInputGetState`).
pub(crate) fn gamepad(player: usize) -> Gamepad {
    let stick = input(player);
    // x +right, y +up
    let x = axis(stick.axis(Axis::LeftX), stick.pressed(button::LEFT), stick.pressed(button::RIGHT));
    let y = axis(stick.axis(Axis::LeftY).saturating_neg(), stick.pressed(button::DOWN), stick.pressed(button::UP));
    // the game's joystick takes winsavage's LY as its x and LX as its y (inverted)
    let mut pad = Gamepad { lx: y.saturating_neg(), ly: x, ..Default::default() };
    map().for_each_pressed(&stick, |n| {
        pad.buttons |= match n {
            Native::Bite => XINPUT_GAMEPAD_A,
            Native::Roar => XINPUT_GAMEPAD_X,
            Native::Run => XINPUT_GAMEPAD_Y,
            Native::Start => XINPUT_GAMEPAD_START,
        }
    });
    pad
}
