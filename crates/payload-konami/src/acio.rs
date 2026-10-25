//! The ACIO bus: started at once, every device present (the game probes device 1, the card
//! reader, and device 5, the HBHI board).

use std::ffi::c_char;

use wal_payload_common::log;

/// `ac_io_is_active2` status of a bus whose node enumeration completed.
const BUS_READY: u32 = 6;

/// `(port, version, out, flags)`; Metal Gear Arcade passes `(port, device, version, out,
/// flags)`: the version is the second argument unless it is a small number (cdecl: reading a
/// fifth argument is harmless).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_begin(_a: u32, b: usize, c: usize, d: usize, _e: u32) -> bool {
    let (version, out) = if b < 0x10000 { (c as *const c_char, d as *mut u32) } else { (b as *const c_char, c as *mut u32) };
    let version = if version.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(version) }.to_string_lossy().into_owned()
    };
    log!("acio: begin (game {version})");
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_begin_get_status(_mask: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_end(_a: u32) -> bool {
    log!("acio: end");
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_end_get_status(_a: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_update() {}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_is_active2(_a: u32, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = BUS_READY };
    }
    true
}

/// The older call (Winning Eleven 2008): at least `nodes` devices answer.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_is_active(_nodes: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_is_active_device(_device: u32) -> bool {
    true
}

/// A device's version record (0x2c bytes): firmware 2.0.0 at bytes 5-7 (spice2x's answer: some
/// games check it), the rest zeros.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_get_version(out: *mut u8, device: u32) -> bool {
    if !out.is_null() {
        unsafe {
            std::ptr::write_bytes(out, 0, 0x2c);
            *out.add(5) = 2;
        }
    }
    log!("acio: device {device} present");
    true
}

/// No device waits for a firmware update.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_get_firmware_update_device_index() -> u8 {
    0xff
}

/// AVS booted by libacio for its own use (Otomedius): libacio is this payload, which needs none.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_avs_boot(_mode: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_avs_shutdown() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_go_firmware_update() -> bool {
    true
}

/// The serial line's statistics (0x58 bytes of counters): zeros, nothing lost.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_get_rs232c_status(out: *mut u8) {
    if !out.is_null() {
        unsafe { std::ptr::write_bytes(out, 0, 0x58) };
    }
}

/// The bus's send / receive logs: empty.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_get_send_log(_out: *mut u8, _size: u32) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_get_recv_log(_out: *mut u8, _size: u32) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_reset() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_set_get_status_device(_device: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_soft_watch_dog_on(_time: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_soft_watch_dog_off() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_set_soft_watch_dog(_time: u32) -> bool {
    true
}

/// The board's game I/Os (Contra: Evolution's start): on.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_enable_game_ios(_on: bool) -> bool {
    true
}

/// The security plugs' password (Contra: Evolution): the emulated plugs need none.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_secplug_set_encodedpasswd(_password: *const u8, _size: i32) {}
