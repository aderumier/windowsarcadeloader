//! Raw Thrills PC cabinet payload, loaded by `wal-loader` (the games read their I/O board
//! themselves, no driver DLL to replace).
//!
//! The cabinets have a JAMMA USB I/O board (a HID device: buttons, coins, wheel and pedals,
//! lamps) and a USB watchdog. The games open the board through SetupAPI/HID and parse its
//! reports in an I/O thread of their own.
//!
//! * `sdaemon.rs`: the `sdaemon.exe` games, The Fast and the Furious and its Super Bikes
//!   (`WAL_RAWTHRILLS_GAME`): the board's reports built from the virtual sticks and handed to
//!   the game's report parser.
//! * `h2o.rs`: the H2O board games, The Fast and the Furious SuperCars: the inputs through the
//!   game's `SetControl`.
//! * `D:\` (logs, personal bests) redirected to `WindowsLoader` (`WAL_RAWTHRILLS_DDRIVE`).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod common;
mod h2o;
mod sdaemon;

use std::ffi::c_void;

use wal_payload_common::{drive, log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("rawthrills");
        patches::apply();
        drive::init("WAL_RAWTHRILLS_DDRIVE", "WindowsLoader");
        let game = std::env::var("WAL_RAWTHRILLS_GAME").unwrap_or_default();
        if !h2o::init(game.trim()) {
            sdaemon::init();
        }
        log!("rawthrills: initialized");
    }
    TRUE
}
