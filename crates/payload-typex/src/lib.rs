//! Taito Type X payload, loaded by `wal-loader` (Type X games import no driver DLL).
//!
//! * JVS I/O board on the serial port (`WAL_TYPEX_JVS_PORT`, default `COM2`), fed by the
//!   virtual arcade sticks (`jvs.rs`, in Taito
//!   stick mode).
//! * `D:\` redirected to a game folder (`WAL_TYPEX_DDRIVE`, default `WindowsLoader`).
//! * Profile code patches (`WAL_PATCHES`).
//! * Lightgun games (`WAL_TYPEX_GUNS`): guns written in the game's memory (`guns.rs`).
//! * Video for Windows codecs shipped with the game (`WAL_VFW_CODECS`).

#![allow(non_snake_case)]

mod guns;
mod jvs;

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
        guns::init();
        wal_payload_common::vfw::init();
        log!("typex: initialized");
    }
    TRUE
}
