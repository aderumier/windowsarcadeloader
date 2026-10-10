//! Taito Type X payload, loaded by `wal-loader` (Type X games import no driver DLL).
//!
//! * JVS I/O board on the serial port (`WAL_TYPEX_JVS_PORT`, default `COM2`), fed by the
//!   virtual arcade sticks (`jvs.rs`, in Taito
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
//! * Video for Windows codecs shipped with the game (`WAL_VFW_CODECS`).

#![allow(non_snake_case)]

mod ffb;
mod guns;
mod immersion;
mod jvs;
mod mahjong;
mod medal;
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
        jvs::init();
        medal::init();
        wheel::init();
        mahjong::init();
        guns::init();
        ffb::init();
        touch::init();
        sram::init();
        wal_payload_common::vfw::init();
        log!("typex: initialized");
    }
    TRUE
}
