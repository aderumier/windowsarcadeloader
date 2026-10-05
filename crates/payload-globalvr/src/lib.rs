//! Global VR payload (Far Cry Paradise Lost).
//!
//! The game imports `USBIOExtreme.dll` (driver of Global VR's USBIO board: gun positions,
//! gun and panel buttons, coin counters) statically: this DLL takes its place in the run
//! directory, so it loads before the game code without any injector.
//!
//! * `usbio.rs`: the `CUSBIO` class the game calls, fed by the virtual arcade sticks.
//! * `W:\` redirected to the game directory (the cabinet runs the game from `subst W: .`):
//!   `WAL_GLOBALVR_WDRIVE`, default `.`.
//! * `g_arcadeError` of `systemcfg.lua` reset at load (`WAL_GLOBALVR_CLEAR_ERRORS=0` keeps it).
//! * `hasp.rs`: the game's HASP4 dongle calls (`record` feature: log WindowsLoader's answers).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]
// recording builds leave the emulation out
#![cfg_attr(feature = "record", allow(dead_code, unused_imports))]

mod config;
mod hasp;
mod usbio;

use std::ffi::c_void;

use wal_payload_common::{drive, log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use usbio::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("globalvr");
        patches::apply();
        // recording builds run under WindowsLoader: only log, change nothing else
        #[cfg(not(feature = "record"))]
        {
            drive::init_letter(b'W', "WAL_GLOBALVR_WDRIVE", ".");
            if std::env::var("WAL_GLOBALVR_CLEAR_ERRORS").map_or(true, |v| v != "0") {
                config::clear_errors();
            }
        }
        usbio::init();
        hasp::init();
        log!("globalvr: initialized");
    }
    TRUE
}
