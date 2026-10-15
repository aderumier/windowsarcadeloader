//! Sega RingEdge payload, loaded by `wal-loader` (the games link Sega's amLib statically and
//! import no driver DLL).
//!
//! * The kernel drivers amLib opens: SRAM, SMBus (EEPROM, DIP switches), super I/O, DMI
//!   (`devices.rs`), saved in `WAL_RINGEDGE_SAVE` (default `sv`).
//! * The system services it asks over PCP: keychip and master (`pcp.rs`).
//! * The JVS I/O board (Sega 837-14572) on the JVS driver `\\.\mxjvs` (`WAL_TYPEX_JVS_PORT`:
//!   `COM4` for games using the serial port), the board shared with Type X (payload-common
//!   `jvs_board.rs`).
//! * amLib's own log (`WAL_RINGEDGE_AMLOG`, `amlog.rs`).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod amlog;
mod devices;
mod pcp;

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use wal_payload_common::{log, patches, serial};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("ringedge");
        patches::apply();
        amlog::init();
        // JVS sense line: amJvs reads DSR (0x20) as "board addressed"
        serial::MODEM_STATUS[0].store(0x10, Ordering::Relaxed);
        serial::MODEM_STATUS[1].store(0x30, Ordering::Relaxed);
        wal_payload_common::jvs_board::init("mxjvs");
        devices::init();
        pcp::start();
        log!("ringedge: initialized");
    }
    TRUE
}
