//! Cars (2012 build, the 4 GB patched `sdaemon.exe`), `WAL_RAWTHRILLS_GAME=cars`.
//!
//! The game reads its I/O board, the RIO (USB `VID_0C70&PID_0780`), through libusb0: a reader
//! thread of its own takes 64-byte interrupt packets (`0x819d10`) and hands them to its parser
//! (`0x818560`, by type in byte 0). The libusb0 imports are answered here instead: one bus
//! with the RIO on it, every call succeeding, the game's commands (lamps, its handshake)
//! accepted, each read returning a type 2 packet built from the virtual sticks.
//!
//! A type 2 packet (after its 3-byte header) holds the switches as 4-bit transition counters
//! (odd: pressed), two a byte, high nibble first, and the 12-bit analog channels:
//! * switches: 0 coin 1, 1 coin 2 (byte 0); 2 bill, 0x12 (byte 1); 4 TEST, 3 SERVICE (byte 2);
//!   5 / 6 volume up / down (byte 3); 7 START, 8 VIEW (byte 5); 9 flip, 0xa jump (byte 6);
//!   0xb chatter, 0xc (byte 7); 0x11 (byte 8); 0xd, 0xe (byte 9); 0xf, 0x10 (byte 10);
//! * analogs 0x13 steering, 0x14 gas, 0x15 brake, 0x16: high nibbles in bytes 11-12, low
//!   bytes in 13-16 (the test menu's calibration maps them).
//!
//! The game's dongle (a HASP key, its statically linked API) answered here too: login
//! succeeds, its memory (serial number at 0x28, a counter at 0x2c) reads from a 256-byte
//! image (serial `WAL_CARS_SERIAL`, default 1), writes go to it, encrypting and decrypting
//! leave the data as is (the game encrypts "Flynn Lives" at start and checks it decrypts back
//! every few seconds: without a dongle it reboots).
//!
//! Force feedback: the wheel force the race (or the test menu) sets, a float at
//! `[[0x18ae0d8] + 0x1b4] + 0x258` (-1..1, positive pushing left).

use wal_payload_common::log;

use std::sync::Mutex;

use crate::common::{Site, matches, redirect, write};

/// The game's state object holding the wheel force (`+0x1b4` -> `+0x258`).
const STATE: usize = 0x18ae0d8;

/// Board switch ids.
const NATIVES: &[(&str, u8)] = &[
    ("coin1", 0x00),
    ("coin2", 0x01),
    ("bill", 0x02),
    ("service", 0x03),
    ("test", 0x04),
    ("volup", 0x05),
    ("voldown", 0x06),
    ("start", 0x07),
    ("view", 0x08),
    ("flip", 0x09),
    ("jump", 0x0a),
    ("chatter", 0x0b),
];

/// Virtual stick -> board, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "view"),
    ("b2", "jump"),
    ("b3", "flip"),
    ("b4", "chatter"),
    ("b5", "bill"),
    ("up", "volup"),
    ("down", "voldown"),
];

/// The steering and gas values scaled by the calibration are clamped to 0xfff with a signed
/// compare: an infinite scale (minimum = maximum, while calibrating) gives a negative value
/// that indexed far out of the game's response table (crash). Unsigned compares.
const PATCHES: &[(Site, &[u8])] = &[
    ((0x81ef22, &[0x7e, 0x07, 0xc7, 0x45, 0xf0, 0xff, 0x0f]), &[0x76]),
    ((0x81f2ee, &[0x7e, 0x07, 0xc7, 0x45, 0xf0, 0xff, 0x0f]), &[0x76]),
];

/// The HASP API functions (stdcall), their entry checked: login, logout, read, write,
/// decrypt, encrypt.
const HASP: [Site; 6] = [
    (0x147f67d, &[0x56, 0x8b, 0x74, 0x24, 0x0c, 0x85, 0xf6]),
    (0x14959f0, &[0x56, 0x8b, 0x74, 0x24, 0x08, 0x6a, 0x00]),
    (0x1494504, &[0x57, 0x8b, 0x7c, 0x24, 0x18, 0x85, 0xff]),
    (0x14c049f, &[0x57, 0x8b, 0x7c, 0x24, 0x18, 0x85, 0xff]),
    (0x148fd65, &[0x53, 0x8b, 0x5c, 0x24, 0x0c, 0x85, 0xdb]),
    (0x14c8ab7, &[0x53, 0x8b, 0x5c, 0x24, 0x0c, 0x85, 0xdb]),
];

/// The dongle's memory.
static DONGLE: Mutex<[u8; 256]> = Mutex::new([0; 256]);

/// Patches Cars when `WAL_RAWTHRILLS_GAME=cars`.
pub(crate) fn init(name: &str) -> bool {
    if !name.eq_ignore_ascii_case("cars") {
        return false;
    }
    if !crate::rio::init("cars", NATIVES, DEFAULT_MAP, &[], false, wheel_force) {
        return true;
    }
    if PATCHES.iter().all(|(s, _)| matches(*s)) {
        for ((at, _), code) in PATCHES {
            write(*at, code);
        }
    } else {
        log!("cars: unexpected calibration code, left alone");
    }
    if HASP.iter().all(|s| matches(*s)) {
        let serial: u32 = std::env::var("WAL_CARS_SERIAL").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(1);
        DONGLE.lock().unwrap()[0x28..0x2c].copy_from_slice(&serial.to_le_bytes());
        let fakes = [
            hasp_login as *const () as usize,
            hasp_logout as *const () as usize,
            hasp_read as *const () as usize,
            hasp_write as *const () as usize,
            hasp_crypt as *const () as usize,
            hasp_crypt as *const () as usize,
        ];
        for ((at, _), fake) in HASP.iter().zip(fakes) {
            redirect(*at, fake);
        }
        log!("cars: dongle emulated (serial {serial})");
    } else {
        log!("cars: unexpected dongle API code, left alone");
    }
    true
}

unsafe extern "system" fn hasp_login(_feature: u32, _vendor: usize, handle: *mut u32) -> u32 {
    unsafe { handle.write(1) };
    0
}

unsafe extern "system" fn hasp_logout(_handle: u32) -> u32 {
    0
}

/// `hasp_read(handle, file, offset, length, buffer)`
unsafe extern "system" fn hasp_read(_handle: u32, _file: u32, offset: u32, length: u32, buffer: *mut u8) -> u32 {
    let memory = DONGLE.lock().unwrap();
    for i in 0..length as usize {
        let v = memory.get(offset as usize + i).copied().unwrap_or(0);
        unsafe { buffer.add(i).write(v) };
    }
    0
}

/// `hasp_write(handle, file, offset, length, buffer)`
unsafe extern "system" fn hasp_write(_handle: u32, _file: u32, offset: u32, length: u32, buffer: *const u8) -> u32 {
    let mut memory = DONGLE.lock().unwrap();
    for i in 0..length as usize {
        if let Some(m) = memory.get_mut(offset as usize + i) {
            *m = unsafe { buffer.add(i).read() };
        }
    }
    0
}

/// `hasp_encrypt` / `hasp_decrypt(handle, buffer, length)`: the data left as is.
unsafe extern "system" fn hasp_crypt(_handle: u32, _buffer: *mut u8, _length: u32) -> u32 {
    0
}

/// `size_of::<T>()` bytes of the game at `address`, None when unreadable (its objects not
/// created yet).
fn read<T: Copy + Default>(address: usize) -> Option<T> {
    use windows_sys::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
    let mut value = T::default();
    let mut done = 0;
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), address as *const _, (&mut value as *mut T).cast(), size_of::<T>(), &mut done) };
    (ok != 0 && done == size_of::<T>()).then_some(value)
}

fn wheel_force() -> f32 {
    read::<usize>(STATE).and_then(|s| read::<usize>(s + 0x1b4)).and_then(|s| read::<f32>(s + 0x258)).unwrap_or(0.0)
}
