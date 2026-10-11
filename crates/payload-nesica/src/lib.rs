//! NESiCAxLive payload.
//!
//! NESiCA games import `iDmacDrv32.dll` (the Taito Type X I/O driver) statically, so this
//! DLL is dropped in its place in the game run directory: it is loaded before the game
//! entry point without any injector, implements the FastIO driver API on top of the
//! virtual arcade stick, and starts the other emulated services (registry, NESYS).
//!
//! NESiCAxLive 2 games import `NxL2Core.dll` instead: the same DLL is installed under that name
//! (system `nesica2`) and implements its API (`nxl2.rs`) on top of the FastIO emulation.
//!

#![allow(non_snake_case)]

mod crypttrace;
mod drive;
mod fastio;
mod nesys;
mod nesystrace;
mod nxl2;
mod registry;
mod rfid;

use std::ffi::c_void;
use wal_payload_common::{log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use fastio::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("nesica");
        patches::apply();
        // The game reads its settings before opening the I/O driver: write them now.
        registry::init();
        drive::init();
        // after drive: its CreateFile hooks chain in front of the D: redirection
        rfid::init();
        fastio::init();
        crypttrace::init();
        nesystrace::init();
        if std::env::var("WAL_NESICA_NESYS").map_or(true, |v| v != "0") {
            nesys::start();
        }
        if std::env::var("WAL_NESICA_CRYPT").map_or(true, |v| v != "0") {
            wal_payload_common::crypto::start();
        }
        log!("nesica: initialized");
    }
    TRUE
}
