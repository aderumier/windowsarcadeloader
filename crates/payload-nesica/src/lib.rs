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
//! Theatrhythm Final Fantasy All-Star Carnival (Unity) calls three native plugins instead: the
//! same DLL is installed under their names (`theatrhythm.rs`).
//!
//! Tetote X Connect (Unity) is loaded at its executable's entry point; its native plugins'
//! functions are the payload's (`tetote.rs`).

#![allow(non_snake_case)]

mod crypttrace;
mod drive;
mod fastio;
mod grip;
mod hidmouse;
mod lobby;
mod mouse;
mod nesys;
mod nesystrace;
mod nxl2;
mod rfid;
mod theatrhythm;
mod theatrhythm_mono;
mod theatrhythm_nesys;
mod theatrhythm_player;
mod theatrhythm_server;
mod tetote;
mod tetote_server;

use std::ffi::c_void;
use wal_payload_common::{log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use fastio::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("nesica");
        patches::apply();
        wal_payload_common::memdiff::start();
        // The game reads its settings before opening the I/O driver: write them now.
        wal_payload_common::nesica_registry::init();
        drive::init();
        // after drive: its CreateFile hooks chain in front of the D: redirection
        rfid::init();
        fastio::init();
        hidmouse::init();
        grip::init();
        mouse::init();
        lobby::init();
        crypttrace::init();
        nesystrace::init();
        if std::env::var("WAL_NESICA_NESYS").map_or(true, |v| v != "0") {
            nesys::start();
        }
        if std::env::var("WAL_NESICA_CRYPT").map_or(true, |v| v != "0") {
            wal_payload_common::crypto::start();
        }
        if theatrhythm::detected(module) {
            theatrhythm::init();
        }
        if tetote::detected() {
            tetote::init();
        }
        log!("nesica: initialized");
    }
    TRUE
}
