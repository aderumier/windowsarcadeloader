//! Wahlap payload (Storm Racer G, Storm Rider).
//!
//! The game imports `usb_io.dll` (driver of Wahlap's USB I/O board: wheel, pedals, buttons,
//! coin counter, steering motor, lamps, seat) statically: this DLL takes its place in the run
//! directory, so it loads before the game code without any injector.
//!
//! * `usbio.rs`: the `UsbIO_*` functions the game calls, fed by the virtual arcade sticks.
//! * `motobikeio.rs`: Storm Rider's board (`motobikeio.dll`, the same DLL under that name), its
//!   memory in a file, its dongle class answered.
//! * `ffb.rs`: force feedback from the steering motor and road outputs.
//! * `keys.rs`: the game's own keyboard (window messages) from the virtual sticks only, the
//!   real keyboard and XInput pads hidden from it.
//! * Video for Windows codecs shipped with the game (`WAL_VFW_CODECS`: the movies are WMV9 AVIs).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod ffb;
mod keys;
mod motobikeio;
mod usbio;

use std::ffi::c_void;

use wal_payload_common::{log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use usbio::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("wahlap");
        patches::apply();
        wal_payload_common::vfw::init();
        keys::init();
        motobikeio::init();
        usbio::init();
        log!("wahlap: initialized");
    }
    TRUE
}
