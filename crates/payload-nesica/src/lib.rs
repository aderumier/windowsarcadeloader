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
//! Magician's Dead (Unreal) loads its I/O board DLL and the Leap Motion SDK: this DLL is both
//! (`magiciansdead.rs`, `leap.rs`).
//!
//! Street Fighter V Type Arcade (Unreal) loads Taito's USB I/O library: this DLL is it
//! (`sfv.rs`).
//!
//! Groove Coaster 2 (`WAL_NESICA_GAME=groovecoaster`): `groovecoaster.rs`.
//!
//! Gunslinger Stratos 2 / 3 (`WAL_NESICA_GAME=gunslinger`, 64-bit, `iDmacDrv64.dll`): `gunslinger.rs`.
//!
//! Tetote X Connect (Unity) is loaded at its executable's entry point; its native plugins'
//! functions are the payload's (`tetote.rs`).

#![allow(non_snake_case)]

mod crypttrace;
mod drive;
mod fastio;
mod grip;
mod groovecoaster;
mod groovecoaster_server;
mod gunslinger;
mod hidmouse;
mod lobby;
// 64-bit only (Magician's Dead; the Leap SDK's MSVC x64 names)
#[cfg(target_pointer_width = "64")]
mod leap;
#[cfg(target_pointer_width = "64")]
mod magiciansdead;
#[cfg(target_pointer_width = "64")]
mod magiciansdead_server;
mod mouse;
mod nesys;
mod nesystrace;
mod nxl2;
mod rfid;
// 64-bit only (Street Fighter V Type Arcade)
#[cfg(target_pointer_width = "64")]
mod sfv;
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
        #[cfg(target_pointer_width = "64")]
        if magiciansdead::detected(module) {
            magiciansdead::init();
        }
        #[cfg(target_pointer_width = "64")]
        if sfv::detected(module) {
            sfv::init();
        }
        if tetote::detected() {
            tetote::init();
        }
        if groovecoaster::detected() {
            groovecoaster::init();
        }
        if gunslinger::detected() {
            gunslinger::init();
        }
        log!("nesica: initialized");
    }
    TRUE
}
