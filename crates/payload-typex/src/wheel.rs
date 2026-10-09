//! Steering board of Taito Type X driving cabinets (Valve Limit R): the wheel's motor
//! (force feedback) driver on a serial port, which also reports the wheel position.
//!
//! `WAL_TYPEX_WHEEL_PORT=COM1` enables it. The game writes 2-byte commands (38400 baud) and
//! reads a 2-byte reply to each one:
//! * `20 xx` reset: `A0 00` (ready).
//! * `1F xx` motor stop, before the position reports: `1F 00` (bit 7 clear: the board is
//!   already calibrated, the game skips its wheel calibration).
//! * `11 xx` starts the position reports: from then on every command (the motor forces) is
//!   answered with the position, `0x400 | pos` big endian (pos 10 bits, 0 = right; the game
//!   takes the low 10 bits when the first byte has a bit in 0x0C).
//! * other commands before that: `8C A0` (status after calibration).
//!
//! Wheel = player 1's `lx` (`WAL_TYPEX_WHEEL_AXIS`, `-` inverts). Forces are not sent to the
//! launcher (outputs TODO).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wal_payload_common::{log, serial};
use wal_protocol::Axis;

static REPORTING: AtomicBool = AtomicBool::new(false);
/// The wheel axis (`Axis` index) and bit 7 set when inverted.
static AXIS: AtomicU8 = AtomicU8::new(Axis::LeftX as u8);

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_TYPEX_WHEEL_PORT") else { return };
    if let Ok(name) = std::env::var("WAL_TYPEX_WHEEL_AXIS") {
        let name = name.trim();
        let (axis, inverted) = name.strip_prefix('-').map_or((name, false), |n| (n, true));
        match Axis::from_name(axis) {
            Some(a) => AXIS.store(a as u8 | if inverted { 0x80 } else { 0 }, Ordering::Relaxed),
            None => log!("wheel: unknown axis {name:?}"),
        }
    }
    serial::install(port.trim(), process);
    log!("wheel: steering board on {}", port.trim());
}

/// Wheel position, 10 bits: 0 = full right, 1023 = full left.
fn position() -> u16 {
    let a = AXIS.load(Ordering::Relaxed);
    let axis = Axis::ALL[(a & 0x7F) as usize];
    let v = wal_payload_common::input(0).axis_u16(axis);
    let v = if a & 0x80 != 0 { !v } else { v };
    (!v) >> 6
}

fn process(packet: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(packet.len());
    for cmd in packet.chunks(2) {
        let reply = match cmd[0] {
            0x20 => {
                REPORTING.store(false, Ordering::Relaxed);
                [0xA0, 0x00]
            }
            0x1F if !REPORTING.load(Ordering::Relaxed) => [0x1F, 0x00],
            c => {
                if c == 0x11 && !REPORTING.swap(true, Ordering::Relaxed) {
                    log!("wheel: position reports started");
                }
                if REPORTING.load(Ordering::Relaxed) { (0x400 | position()).to_be_bytes() } else { [0x8C, 0xA0] }
            }
        };
        out.extend_from_slice(&reply);
    }
    out
}
