//! Taito Type X payload, loaded by `wal-loader` (Type X games import no driver DLL).
//!
//! * JVS I/O board on the serial port (`WAL_TYPEX_JVS_PORT`, default `COM2`), fed by the
//!   virtual arcade sticks (payload-common `jvs_board.rs`, in Taito
//!   stick mode).
//! * `D:\` redirected to a game folder (`WAL_TYPEX_DDRIVE`, default `WindowsLoader`).
//! * Profile code patches (`WAL_PATCHES`).
//! * Lightgun games (`WAL_TYPEX_GUNS`): guns written in the game's memory (`guns.rs`).
//! * Block King Ball Shooter's touch sensor driver (`WAL_TYPEX_LSDRV`, `touch.rs`).
//! * The medal games' TXE001 backup SRAM board (`TxedLap.dll` imports, `sram.rs`).
//! * Medal I/O board on a serial port (`WAL_TYPEX_MEDAL_PORT`, `medal.rs`).
//! * Steering (wheel motor) board of driving games on a serial port (`WAL_TYPEX_WHEEL_PORT`,
//!   `wheel.rs`).
//! * Force feedback computed from the game's memory (`WAL_TYPEX_FFB`, `ffb.rs`).
//! * Mahjong panel read as a keyboard (`WAL_TYPEX_MAHJONG`, `mahjong.rs`).
//! * Elevator Action Death Parade's attraction doors drawn over the picture (`WAL_TYPEX_DOORS`,
//!   `doors.rs`).
//! * Sonic Blast Heroes' punching pad from a gamepad trigger (`WAL_TYPEX_PUNCH`, `punch.rs`).
//! * Video for Windows codecs shipped with the game (`WAL_VFW_CODECS`).

#![allow(non_snake_case)]

mod doors;
mod ffb;
mod guns;
mod immersion;
mod mahjong;
mod medal;
mod punch;
mod sram;
mod touch;
mod wheel;

use std::ffi::c_void;

use wal_payload_common::{drive, log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("typex");
        patches::apply();
        drive::init("WAL_TYPEX_DDRIVE", "WindowsLoader");
        wal_payload_common::jvs_board::init("COM2");
        medal::init();
        wheel::init();
        mahjong::init();
        guns::init();
        doors::init();
        ffb::init();
        touch::init();
        punch::init();
        sram::init();
        wal_payload_common::vfw::init();
        // Taito crypto service (KOF XIII Climax): only for a profile naming its key
        if wal_payload_common::crypto::key_configured() {
            wal_payload_common::crypto::start();
        }
        log!("typex: initialized");
    }
    TRUE
}
