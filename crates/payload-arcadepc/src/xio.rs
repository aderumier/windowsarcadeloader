//! Frenzy Express's I/O library (`xio.dll`, `WAL_ARCADEPC_BOARD=xio`): the payload is it
//! (`payload_install`, x86 build only).
//!
//! `xwin.exe` imports a C++ class `XIO` (thiscall methods, MSVC names in `xio.def`) and bytes it
//! reads after each `update` (60 per second): `g_bKeyImm` the keys held, `g_bKeyExc` the keys
//! pressed since the last update (bit 0 start, 1 service, 2 test, 3 coin, 4 left, 5 right),
//! `g_bHandle` the handlebar (0 left, 0x80 center, 0xFE right), `g_bBrake` the brake (0x80
//! half, 0xFF full), and the two pedals: per pedal its position change since the last update
//! (`g_bLAccelDif`, `g_bRAccelDif`), whether that change grows (`g_bLAccel`, `g_bRAccel`) and
//! the position (`g_bLAccelEnd`, `g_bRAccelEnd`). The game pedals by turns: the pedal expected
//! next moving drives the scooter (`g_bAccelerator`, written by the game), the other one
//! switches. Its demo writes these bytes too (exported data, writable).
//!
//! The pedals are keys as the dump's keyboard library: a pedal held goes down 2 per update (to
//! 0xFE), released comes back up 2. `pedal` (the accelerator pedal, `accel` past a quarter)
//! presses them by turns. The other methods (lamps, vibration, the interface) do nothing.
//!
//! Natives: `start service test coin left right pedal_left pedal_right pedal brake
//! brake_half`; the left stick steers.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use wal_protocol::Axis;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Service,
    Test,
    Coin,
    Left,
    Right,
    PedalLeft,
    PedalRight,
    Pedal,
    Brake,
    BrakeHalf,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("service", Native::Service),
    ("test", Native::Test),
    ("coin", Native::Coin),
    ("left", Native::Left),
    ("right", Native::Right),
    ("pedal_left", Native::PedalLeft),
    ("pedal_right", Native::PedalRight),
    ("pedal", Native::Pedal),
    ("brake", Native::Brake),
    ("brake_half", Native::BrakeHalf),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("service", "service"),
    ("test", "test"),
    ("coin", "coin"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "pedal"),
    ("b2", "brake"),
    ("b3", "pedal_left"),
    ("b4", "pedal_right"),
    ("b5", "brake_half"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

// The exported bytes (`xio.def`).
#[unsafe(no_mangle)]
pub static XIO_KEY_IMM: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_KEY_EXC: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_HANDLE: AtomicU8 = AtomicU8::new(0x80);
#[unsafe(no_mangle)]
pub static XIO_BRAKE: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_ACCELERATOR: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_L_ACCEL: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_R_ACCEL: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_L_ACCEL_DIF: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_R_ACCEL_DIF: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_L_ACCEL_DIF_LAST: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_R_ACCEL_DIF_LAST: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_L_ACCEL_END: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_R_ACCEL_END: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_L_PEDAL: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_R_PEDAL: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_L_PEDAL_LAST: AtomicU8 = AtomicU8::new(0);
#[unsafe(no_mangle)]
pub static XIO_R_PEDAL_LAST: AtomicU8 = AtomicU8::new(0);

/// `pedal`'s update count: each pedal's turn lasts 6 updates (`accel`: 18 at a quarter, 6 full).
static AUTO_TICK: AtomicU8 = AtomicU8::new(0);

/// A pedal's new position: down 2 held, up 2 released.
fn pedal_step(position: u8, held: bool) -> u8 {
    if held { position.saturating_add(2).min(0xFE) } else { position.saturating_sub(2) }
}

/// The handlebar: `analog` the left stick (-32768..32767), else the keys move it 0x20 per
/// update, back to the center released.
fn handle_step(handle: u8, left: bool, right: bool, analog: i16) -> u8 {
    let h = handle as i32;
    let v = if left && !right {
        h - 0x20
    } else if right && !left {
        h + 0x20
    } else if analog.unsigned_abs() > 2048 {
        0x80 + analog as i32 * 0x7E / 32767
    } else if h > 0x80 {
        (h - 0x20).max(0x80)
    } else {
        (h + 0x20).min(0x80)
    };
    v.clamp(0, 0xFE) as u8
}

/// The pedals' bytes after a move (the dump's library's `updatePedalState`).
fn pedal_state(position: u8, last: &AtomicU8, dif_last: &AtomicU8, dif: &AtomicU8, accel: &AtomicU8, end: &AtomicU8) {
    let d = (position as i32 - last.swap(position, Ordering::Relaxed) as i32).unsigned_abs() as u8;
    end.store(position, Ordering::Relaxed);
    dif.store(d, Ordering::Relaxed);
    accel.store((dif_last.swap(d, Ordering::Relaxed) < d) as u8, Ordering::Relaxed);
}

type This = *mut u8;

#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_new(this: This) -> This {
    log!("xio: XIO object at {this:p}");
    this
}

#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_none(_this: This) {}

#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_none1(_this: This, _a: u32) {}

#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_none2(_this: This, _a: u32, _b: u32) {}

/// `openInterface`: the board answers.
#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_open(_this: This) -> i32 {
    log!("xio: openInterface");
    1
}

#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_handle(_this: This) -> u8 {
    XIO_HANDLE.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_zero(_this: This) -> u8 {
    0
}

/// `resetPedal`, `initPedal`: both pedals up.
#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_reset_pedals(_this: This) {
    for b in [&XIO_L_PEDAL, &XIO_R_PEDAL, &XIO_L_PEDAL_LAST, &XIO_R_PEDAL_LAST, &XIO_L_ACCEL_END, &XIO_R_ACCEL_END] {
        b.store(0, Ordering::Relaxed);
    }
    for b in [&XIO_L_ACCEL_DIF, &XIO_R_ACCEL_DIF, &XIO_L_ACCEL_DIF_LAST, &XIO_R_ACCEL_DIF_LAST, &XIO_L_ACCEL, &XIO_R_ACCEL] {
        b.store(0, Ordering::Relaxed);
    }
}

/// `update`: the inputs of the last 1/60 s.
#[unsafe(no_mangle)]
pub extern "thiscall" fn xio_update(_this: This) {
    let stick = input(0);
    let mut held = [false; NATIVES.len()];
    map().for_each_pressed(&stick, |n| held[n as usize] = true);
    let on = |n: Native| held[n as usize];
    let lx = stick.axis(Axis::LeftX);

    // the keys; the stick past half way is left / right too (menus)
    let keys = [
        (Native::Start, 0x01),
        (Native::Service, 0x02),
        (Native::Test, 0x04),
        (Native::Coin, 0x08),
        (Native::Left, 0x10),
        (Native::Right, 0x20),
    ];
    let mut imm = keys.iter().filter(|(n, _)| on(*n)).fold(0u8, |k, (_, bit)| k | bit);
    if lx < -16384 {
        imm |= 0x10;
    }
    if lx > 16384 {
        imm |= 0x20;
    }
    let was = XIO_KEY_IMM.swap(imm, Ordering::Relaxed);
    XIO_KEY_EXC.store(imm & !was, Ordering::Relaxed);

    let handle = handle_step(XIO_HANDLE.load(Ordering::Relaxed), on(Native::Left), on(Native::Right), lx);
    XIO_HANDLE.store(handle, Ordering::Relaxed);

    let analog = stick.axis_u8(Axis::Brake);
    let brake = if on(Native::Brake) {
        0xFF
    } else if on(Native::BrakeHalf) {
        0x80.max(analog)
    } else {
        analog
    };
    XIO_BRAKE.store(brake, Ordering::Relaxed);

    // `pedal`: the left pedal, then the right one, each for a turn of `turn` updates
    let accel = stick.axis(Axis::Accel);
    let auto = on(Native::Pedal) || accel > 8192;
    let (mut left, mut right) = (on(Native::PedalLeft), on(Native::PedalRight));
    if auto {
        let turn = if on(Native::Pedal) { 6 } else { 18 - (accel as i32 - 8192) * 12 / (32767 - 8192) } as u8;
        let tick = AUTO_TICK.load(Ordering::Relaxed).wrapping_add(1) % (2 * turn);
        AUTO_TICK.store(tick, Ordering::Relaxed);
        if tick < turn {
            left = true;
        } else {
            right = true;
        }
    } else {
        AUTO_TICK.store(0, Ordering::Relaxed);
    }
    let l = pedal_step(XIO_L_PEDAL.load(Ordering::Relaxed), left);
    let r = pedal_step(XIO_R_PEDAL.load(Ordering::Relaxed), right);
    XIO_L_PEDAL.store(l, Ordering::Relaxed);
    XIO_R_PEDAL.store(r, Ordering::Relaxed);
    pedal_state(l, &XIO_L_PEDAL_LAST, &XIO_L_ACCEL_DIF_LAST, &XIO_L_ACCEL_DIF, &XIO_L_ACCEL, &XIO_L_ACCEL_END);
    pedal_state(r, &XIO_R_PEDAL_LAST, &XIO_R_ACCEL_DIF_LAST, &XIO_R_ACCEL_DIF, &XIO_R_ACCEL, &XIO_R_ACCEL_END);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle() {
        assert_eq!(handle_step(0x80, true, false, 0), 0x60);
        assert_eq!(handle_step(0x10, true, false, 0), 0);
        assert_eq!(handle_step(0xF0, false, true, 0), 0xFE);
        assert_eq!(handle_step(0x30, false, false, 0), 0x50);
        assert_eq!(handle_step(0x80, false, false, 32767), 0xFE);
        assert_eq!(handle_step(0x80, false, false, -32768), 0x02);
    }

    #[test]
    fn pedal() {
        assert_eq!(pedal_step(0xFC, true), 0xFE);
        assert_eq!(pedal_step(0xFE, true), 0xFE);
        assert_eq!(pedal_step(1, false), 0);
    }
}
