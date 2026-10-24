//! Action Deka (GDK, 2008): the GDKI action controllers on the ACIO bus (devices 7 to 10, each
//! player's two hand controllers) and the cabinet's system plug on the card reader (device 2).
//!
//! Each controller has a 3-axis accelerometer and three buttons:
//! `ac_io_gdki_get_control_status_buffer` fills 0x30 bytes: +0 a 16-bit status, +4 the 6 raw
//! bytes (3 pairs: `hi & 0x1f` << 7 | `lo & 0x7f`, a signed 12-bit axis; bit 6 of each high
//! byte a button), +0xc/+0x10/+0x14 x, y, z (i32), +0x18 the sample count, +0x20 its time.

use std::sync::atomic::{AtomicU32, Ordering};

use wal_payload_common::log;

/// The first GDKI device and the controllers.
const FIRST: u32 = 7;
const CONTROLLERS: u32 = 4;

static SAMPLES: AtomicU32 = AtomicU32::new(0);

/// A controller's acceleration (x, y, z: signed 12-bit) and buttons (bits 0-2): at rest, no
/// button. The players' motions and buttons come from the game's debug input (`deka.rs`).
fn controller(_index: u32) -> ([i32; 3], u8) {
    ([0, 0, 0], 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_gdki_update_control_status_buffer(device: u32) -> bool {
    (FIRST..FIRST + CONTROLLERS).contains(&device)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_gdki_get_control_status_buffer(device: u32, out: *mut u8) {
    if out.is_null() || !(FIRST..FIRST + CONTROLLERS).contains(&device) {
        return;
    }
    let (accel, buttons) = controller(device - FIRST);
    let mut raw = [0u8; 6];
    for (i, v) in accel.iter().enumerate() {
        let v = (*v).clamp(-2048, 2047) as u32 & 0xfff;
        raw[2 * i] = ((v >> 7) & 0x1f) as u8 | if buttons & (1 << i) != 0 { 0x40 } else { 0 };
        raw[2 * i + 1] = (v & 0x7f) as u8;
    }
    let count = SAMPLES.fetch_add(1, Ordering::Relaxed);
    let mut buf = [0u8; 0x30];
    buf[4..10].copy_from_slice(&raw);
    for (i, v) in accel.iter().enumerate() {
        buf[0xc + 4 * i..0x10 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    buf[0x18..0x1c].copy_from_slice(&count.to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
}

/// The controllers' outputs (channels 0, 1, 2, 5: lamps, vibration): no device.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_gdki_set_output_level(_device: u32, _channel: u32, _level: u8) {}

// The system plug, on the card reader.

/// A plug found.
const PLUG_FOUND: u32 = 0x14;

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_req_secplug_check_systemplug(device: u32) -> bool {
    log!("gdki: system plug check (device {device}): present");
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_req_secplug_check_isfinished(_device: u32, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = PLUG_FOUND };
    }
    true
}

/// The plug's id: 10 wide characters.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_systemid(_device: u32, out: *mut u16) -> bool {
    if out.is_null() {
        return false;
    }
    for (i, c) in "0123456789".encode_utf16().chain([0]).enumerate() {
        unsafe { *out.add(i) = c };
    }
    true
}
