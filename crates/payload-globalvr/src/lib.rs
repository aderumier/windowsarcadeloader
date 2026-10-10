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
//! * `hasp.rs`: the game's HASP4 dongle calls.
//! * `aliens.rs`: Aliens Extermination's guns and panel (`WAL_GLOBALVR_ALIENS=1`).
//! * `keyboard.rs`: the game's own keyboard reduced to the operator keys (test menu).
//! * `swarm.rs`: The Swarm's input adapter (shared memory), `WAL_GLOBALVR_SWARM=1`, and its
//!   stage changes (`swarm_campaign.rs`: the game started again with the next map).
//! * `gamelog.rs`: the game's own messages in the payload log (`WAL_GLOBALVR_GAMELOG=1`).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod aliens;
mod config;
mod gamelog;
mod hasp;
mod keyboard;
mod swarm;
mod swarm_campaign;
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
        drive::init_letter(b'W', "WAL_GLOBALVR_WDRIVE", ".");
        if std::env::var("WAL_GLOBALVR_CLEAR_ERRORS").map_or(true, |v| v != "0") {
            config::clear_errors();
        }
        usbio::init();
        aliens::init();
        hasp::init();
        keyboard::init();
        gamelog::init();
        swarm::init();
        log!("globalvr: initialized");
    }
    TRUE
}
