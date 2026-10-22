//! The HGTH board (GTI Club: Supermini Festa!): the steering wheel's motor board, with the wheel
//! and pedal potentiometers, on its own ACIO bus (`dllibacio.dll`, `gti_club.rs`).
//!
//! `ac_io_hgth_get_recvdata` gives the 32-byte receive buffer: the board's 10-byte packet, its
//! 16-bit words swapped to little endian; +2 the wheel, +4 the accelerator, +6 the brake. The
//! game turns them into -1..1 / 0..1 with its calibration (min, max, wheel center: three u32
//! arrays at 0x918c74, 0x918c68, 0x918c14, defaults set at 0x440660, the test menu's
//! calibration loaded over them): the sticks are scaled onto the calibration the game has, so
//! any calibration steers over the whole wheel range.
//!
//! `ac_io_hgth_set_senddata` sends the motor's 4 bytes (0x918cbc, built at 0x5475d0): byte 0 the
//! torque, -127..127, against the wheel's turn in a race (the self-centering, stronger with the
//! turn and the speed: wheel right gives a negative torque, which pushes left); byte 1 bits 0-5 a
//! vibration level (0..63: crashes, hits, decaying), byte 2 its kind, byte 3 a constant level
//! (0x54 in a race). The torque becomes the constant force, the vibration level the vibration
//! (`WAL_KONAMI_FFB`, percent, default 100).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use wal_payload_common::{log, send_output};
use wal_protocol::{Axis, output};

/// The game's calibration arrays (min, max, center; wheel, accelerator, brake).
const CAL_MIN: usize = 0x918c74;
const CAL_MAX: usize = 0x918c68;
const CAL_CENTER: usize = 0x918c14;

fn calibration(array: usize, i: usize) -> u32 {
    unsafe { ((array + i * 4) as *const u32).read_volatile() }
}

/// The raw value the game reads as `t` (wheel -1..1, pedals 0..1).
fn raw(i: usize, t: f32) -> u16 {
    if !crate::gti_club::active() {
        let full = if i == 0 { (t + 1.0) / 2.0 } else { t };
        return (full * 65535.0) as u16;
    }
    let (min, max) = (calibration(CAL_MIN, i) as f32, calibration(CAL_MAX, i) as f32);
    let v = if i == 0 {
        let center = calibration(CAL_CENTER, 0) as f32;
        if t < 0.0 { center + t * (center - min) } else { center + t * (max - center) }
    } else {
        min + t * (max - min)
    };
    v.clamp(0.0, 65535.0) as u16
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hgth_update_recvdata() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hgth_get_recvdata(out: *mut u8) {
    if out.is_null() {
        return;
    }
    let stick = wal_payload_common::input(0);
    let wheel = (stick.axis(Axis::LeftX) as f32 / 32767.0).clamp(-1.0, 1.0);
    let accel = stick.axis(Axis::Accel).max(0) as f32 / 32767.0;
    let brake = stick.axis(Axis::Brake).max(0) as f32 / 32767.0;
    let mut buf = [0u8; 32];
    for (i, t) in [wheel, accel, brake].into_iter().enumerate() {
        buf[2 + i * 2..4 + i * 2].copy_from_slice(&raw(i, t).to_le_bytes());
    }
    unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
}

static PERCENT: OnceLock<i32> = OnceLock::new();
static LAST_SEND: AtomicU32 = AtomicU32::new(0);
static LAST_FORCE: AtomicI32 = AtomicI32::new(0);
static LAST_VIBRATION: AtomicI32 = AtomicI32::new(0);
static LOGGED: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hgth_set_senddata(data: *const u8) -> bool {
    if data.is_null() {
        return true;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, 4) };
    let packed = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if LAST_SEND.swap(packed, Ordering::Relaxed) != packed && LOGGED.fetch_add(1, Ordering::Relaxed) < 30 {
        log!("hgth: motor {bytes:02x?}");
    }
    let percent = *PERCENT.get_or_init(|| {
        std::env::var("WAL_KONAMI_FFB").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(100)
    });
    let force = (bytes[0] as i8 as i32 * output::FFB_MAX / 127 * percent / 100).clamp(-output::FFB_MAX, output::FFB_MAX);
    if LAST_FORCE.swap(force, Ordering::Relaxed) != force {
        send_output(0, output::FFB_CONSTANT, force);
    }
    let vibration = ((bytes[1] & 0x3f) as i32 * output::FFB_MAX / 63 * percent / 100).min(output::FFB_MAX);
    if LAST_VIBRATION.swap(vibration, Ordering::Relaxed) != vibration {
        send_output(0, output::FFB_VIBRATION, vibration);
    }
    true
}

/// The wheel's travel limit (the motor's end stops), applied at once.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hgth_directreq_set_handle_limit(_limit: u8, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 1 };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hgth_directreq_set_handle_limit_isfinished(status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 2 };
    }
    true
}
