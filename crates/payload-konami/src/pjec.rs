//! The PJEC board (Winning Eleven 2008): the cabinet's PlayStation 2 controller port.
//!
//! `ac_io_pjec_get_control_status_buffer` fills 72 bytes; the game reads the first pad: byte 0
//! the device type (7, an analog controller), byte 2 0x5a (a pad present), bytes 4-5 the
//! buttons, bytes 8-11 the analog sticks (left x, y, right x, y; 0x7f centered, 0 left / up), the
//! layout spice2x documents for Winning Eleven's PJEC. The pad is player 1's stick; the board
//! must be present for the cabinet's device check (with the card reader).
//!
//! Native inputs for `native_map`: `pad_start select up down left right triangle cross square
//! circle l1 l2 l3 r1 r2 r3`.

use std::sync::Mutex;

use wal_payload_common::mapping::ButtonMap;
use wal_protocol::Axis;

/// (status byte, bit) of a pad button.
#[derive(Clone, Copy)]
struct Bit(usize, u8);

const NATIVES: &[(&str, Bit)] = &[
    ("pad_start", Bit(4, 0x08)),
    ("select", Bit(4, 0x01)),
    ("up", Bit(4, 0x10)),
    ("down", Bit(4, 0x40)),
    ("left", Bit(4, 0x80)),
    ("right", Bit(4, 0x20)),
    ("l3", Bit(4, 0x02)),
    ("r3", Bit(4, 0x04)),
    ("triangle", Bit(5, 0x10)),
    ("cross", Bit(5, 0x40)),
    ("square", Bit(5, 0x80)),
    ("circle", Bit(5, 0x20)),
    ("l1", Bit(5, 0x04)),
    ("l2", Bit(5, 0x01)),
    ("r1", Bit(5, 0x08)),
    ("r2", Bit(5, 0x02)),
];

/// The virtual buttons as a PlayStation pad (b1 cross, b2 circle, b3 square, b4 triangle, b5 L1,
/// b6 R1, b7 L2, b8 R2). The game reads it once the player chooses the home controller on the
/// touch screen: without touch the arcade stick (PJEI) is the one played.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "pad_start"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "cross"),
    ("b2", "circle"),
    ("b3", "square"),
    ("b4", "triangle"),
    ("b5", "l1"),
    ("b6", "r1"),
    ("b7", "l2"),
    ("b8", "r2"),
];

/// Device type: an analog controller (DualShock 2).
const ANALOG_CONTROLLER: u8 = 7;
/// A controller answers on the port.
const PRESENT: u8 = 0x5a;

static MAP: Mutex<Option<ButtonMap<Bit>>> = Mutex::new(None);
static STATUS: Mutex<[u8; 72]> = Mutex::new([0; 72]);

/// A stick axis (-32768..32767) as the pad's byte (0..255, centered 0x7f).
fn axis(v: i16) -> u8 {
    ((v as i32 + 32768) >> 8) as u8
}

/// A new status from player 1's stick.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjec_update_control_status_buffer() -> bool {
    let stick = wal_payload_common::input(0);
    let mut status = [0u8; 72];
    status[0] = ANALOG_CONTROLLER;
    status[2] = PRESENT;
    status[8] = axis(stick.axis(Axis::LeftX));
    status[9] = axis(stick.axis(Axis::LeftY));
    status[10] = axis(stick.axis(Axis::RightX));
    status[11] = axis(stick.axis(Axis::RightY));
    let mut map = MAP.lock().unwrap();
    map.get_or_insert_with(|| ButtonMap::new(NATIVES, DEFAULT_MAP)).for_each_pressed(&stick, |Bit(byte, bit)| {
        status[byte] |= bit;
    });
    *STATUS.lock().unwrap() = status;
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjec_get_control_status_buffer(out: *mut u8) {
    if !out.is_null() {
        let status = STATUS.lock().unwrap();
        unsafe { std::ptr::copy_nonoverlapping(status.as_ptr(), out, status.len()) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjec_get_ps2() -> bool {
    true
}

/// The pad's vibration motors (port, small, large): no motor here.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pjec_set_motor(_port: u32, _small: u8, _large: u8) -> bool {
    true
}
