//! Storm Rider's I/O board: the functions of `motobikeio.dll` (cdecl, plain names in
//! `usb_io.def`), used by the game and by its `moto_coins.dll` (coins, bookkeeping) and
//! `FirstMotoLightDLL.dll` (lamps).
//!
//! The real driver reads the board's HID reports into a state block its getters return:
//!
//! * `GetControlButton()`: the panel buttons, a byte latched from the board's `55 c d AA`
//!   messages (`c` with bit 7 set); the game keeps bits 0-6 (its button bits 0x01..0x40).
//! * `GetIoKeyData()` / `GetIoKeyState(bit)`: the switch byte, bits 0x01 0x04 0x08 0x10 (the
//!   game's 0x200 0x100 0x20 0x40, as Storm Racer G's `b` word).
//! * `GetADData(ch)`: 10-bit analog channels 0-2 (the real driver inverts channel 0): handlebar,
//!   throttle, brake.
//! * `GetCoin(slot)` / `DecCoin(slot, n)` / `ClearCoin(slot)`: the coin counters (slot 0 = the
//!   emulated counter, `coin_counter`).
//! * `MemoryRead(addr, len, buf, 0)` / `MemoryWrite(..)`: the board's memory, where
//!   `moto_coins.dll` keeps its bookkeeping and settings: `motobikeio.mem` in the game's working
//!   directory (`assets`); channels never calibrated read 0..=1023 (`default_calibration`).
//!
//! The game's input wrapper (`0x433720`) builds its button word from these exactly as Storm
//! Racer G's does. The game's dongle class (`Lock.dll`: `Lock::Initialize`, `Lock::test`) is
//! answered true in its imports.

use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use wal_payload_common::{iat, log};
use wal_protocol::Axis;

const MEMORY_FILE: &str = "motobikeio.mem";
const MEMORY_SIZE: u64 = 0x10000;

static MEMORY: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub(crate) fn init() {
    for name in ["?Initialize@Lock@@QAE_NXZ", "?test@Lock@@QAE_NXZ"] {
        if unsafe { iat::hook("Lock.dll", name, lock_true as *const () as usize) }.is_some() {
            log!("motobikeio: {name} answered true");
        }
    }
}

unsafe extern "thiscall" fn lock_true(_this: *mut u8) -> bool {
    true
}

fn with_memory<T>(f: impl FnOnce(&mut std::fs::File) -> std::io::Result<T>) -> Option<T> {
    let mut memory = MEMORY.lock().unwrap();
    if memory.is_none() {
        match std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(MEMORY_FILE) {
            Ok(file) => {
                if file.metadata().map_or(0, |m| m.len()) < MEMORY_SIZE {
                    let _ = file.set_len(MEMORY_SIZE);
                }
                *memory = Some(file);
            }
            Err(e) => {
                log!("motobikeio: cannot open {MEMORY_FILE}: {e}");
                return None;
            }
        }
    }
    match f(memory.as_mut().unwrap()) {
        Ok(v) => Some(v),
        Err(e) => {
            log!("motobikeio: {MEMORY_FILE}: {e}");
            None
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_init() -> bool {
    log!("motobikeio: InitIoSys");
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_exit() {
    log!("motobikeio: ExitIoSys");
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_true() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_control_button() -> u8 {
    0x80 | (crate::usbio::pressed_bits() & 0x7f) as u8
}

/// `GetPasswordButton()`: the operator keypad's last code, none.
#[unsafe(no_mangle)]
pub extern "C" fn mio_password_button() -> u8 {
    0
}

fn key_data() -> u8 {
    crate::usbio::split(crate::usbio::pressed_bits()).1 as u8
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_key_data() -> i32 {
    key_data() as i8 as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_key_state(bit: i32) -> i32 {
    (key_data() as i32 >> (bit & 7)) & 1
}

/// `GetADData(ch)`: 0..=1023.
#[unsafe(no_mangle)]
pub extern "C" fn mio_ad_data(ch: i32) -> i32 {
    let stick = wal_payload_common::input(0);
    let axis = match ch {
        0 => Axis::LeftX,
        1 => Axis::Accel,
        2 => Axis::Brake,
        _ => return 0,
    };
    (stick.axis_u16(axis) >> 6) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_get_coin(slot: i32) -> i32 {
    if slot == 0 { crate::usbio::coins() as i32 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_dec_coin(slot: i32, n: i32) -> i32 {
    if slot == 0 {
        log!("motobikeio: DecCoin({slot}, {n}): {} left", crate::usbio::take_coins(n.max(0) as u32));
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn mio_clear_coin(slot: i32) -> i32 {
    if slot == 0 {
        crate::usbio::clear_coins();
    }
    1
}

/// `MemoryRead(addr, len, buf, 0)`: 5 (the real driver's success) or 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mio_memory_read(addr: u16, len: u16, buf: *mut u8, _flag: i32) -> i32 {
    if buf.is_null() {
        return 0;
    }
    let data = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    let ok = with_memory(|f| {
        f.seek(SeekFrom::Start(addr as u64))?;
        f.read_exact(data)
    });
    default_calibration(addr, data);
    if ok.is_some() { 5 } else { 0 }
}

/// The operator settings record (`moto_coins.dll` `GetManagerData`), 0x28 bytes at 0x20 of the
/// board memory, holds each analog channel's calibration (u16 min, max at +8, +0xc, +0x10): a
/// channel never calibrated (min >= max: 0, 0 on a new board) reads 0..=1023, the range sent.
const SETTINGS: usize = 0x20;
const CALIBRATION: usize = SETTINGS + 8;

fn default_calibration(addr: u16, data: &mut [u8]) {
    for ch in 0..3 {
        let at = CALIBRATION + ch * 4;
        let Some(off) = at.checked_sub(addr as usize).filter(|o| o + 4 <= data.len()) else { continue };
        let min = u16::from_le_bytes([data[off], data[off + 1]]);
        let max = u16::from_le_bytes([data[off + 2], data[off + 3]]);
        if min >= max {
            data[off..off + 4].copy_from_slice(&[0, 0, 0xff, 0x03]);
            log!("motobikeio: channel {ch} not calibrated ({min}, {max}): 0..1023");
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mio_memory_write(addr: u16, len: u16, buf: *const u8, _flag: i32) -> i32 {
    if buf.is_null() {
        return 0;
    }
    let data = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    let ok = with_memory(|f| {
        f.seek(SeekFrom::Start(addr as u64))?;
        f.write_all(data)?;
        f.flush()
    });
    if ok.is_some() { 5 } else { 0 }
}
