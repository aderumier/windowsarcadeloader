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
//! * A CTR-251500 serial touch panel (`WAL_RINGEDGE_TOUCH`, `touch.rs`).
//! * Profile code patches (`WAL_PATCHES`); `WAL_RINGEDGE_CWD`: the game's working directory,
//!   relative to its own.

#![allow(non_snake_case)]

mod amlog;
mod cg;
mod devices;
mod nvasm;
mod pcp;
mod touch;

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use wal_payload_common::{log, patches, serial};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("ringedge");
        // the game's working directory, relative to its own (its files are found from there)
        if let Ok(dir) = std::env::var("WAL_RINGEDGE_CWD") {
            let ok = std::env::set_current_dir(dir.trim()).is_ok();
            log!("ringedge: working directory {} ({:?})", dir.trim(), if ok { std::env::current_dir().ok() } else { None });
        }
        patches::apply();
        amlog::init();
        // JVS sense line: amJvs reads DSR (0x20) as "board addressed"
        serial::MODEM_STATUS[0].store(0x10, Ordering::Relaxed);
        serial::MODEM_STATUS[1].store(0x30, Ordering::Relaxed);
        wal_payload_common::jvs_board::init("mxjvs");
        // ports the game opens beside its I/O board, present but silent
        if let Ok(ports) = std::env::var("WAL_RINGEDGE_SILENT_PORTS") {
            serial::install_sink(&ports);
        }
        devices::init();
        cg::init();
        touch::init();
        pcp::start();
        log!("ringedge: initialized");
    }
    TRUE
}
