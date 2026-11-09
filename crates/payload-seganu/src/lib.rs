//! Sega Nu payload, loaded by `wal-loader` (the games link Sega's amLib statically and import
//! no driver DLL).
//!
//! * The AMEX board's drivers amLib finds by their device interface classes, the keychip
//!   driver, the hardware monitor and the system's registry values (`devices.rs`), saved in
//!   `WAL_SEGANU_SAVE` (default `sv`).
//! * The JVS I/O board (Sega 837-14572, payload-common's `jvs_board.rs`) on the AMEX JVS
//!   device.
//! * amLib's own log (`WAL_SEGANU_AMLOG`, payload-common's `amlog`).
//! * The test menu program (`TestMode_x86.exe`): started by a relaunch when the game hands over
//!   to it (`devices.rs`); it asks for the game's relaunch, so leaving it starts the game again
//!   (`game.bat`'s loop on the cabinet).
//! * Profile code patches (`WAL_PATCHES`); `WAL_SEGANU_CWD`: the game's working directory,
//!   relative to its own.
//! * 64-bit: a packed executable started once unpacked (`WAL_SEGANU_UNPACK`, `unpack.rs`:
//!   Luigi's Mansion Arcade's Capcom protector).
//! * Luigi's Mansion Arcade's I/O board on COM1 (`WAL_SEGANU_LMAIO`, `lmaio.rs`): guns, coins,
//!   switches.

#![allow(non_snake_case)]

mod devices;
mod lmaio;
#[cfg(target_arch = "x86_64")]
mod unpack;

use std::ffi::c_void;

use wal_payload_common::{log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("seganu");
        // the game's working directory, relative to its own (game.bat runs it from the dump
        // root, its data in fs\)
        if let Ok(dir) = std::env::var("WAL_SEGANU_CWD") {
            let ok = std::env::set_current_dir(dir.trim()).is_ok();
            log!("seganu: working directory {} ({:?})", dir.trim(), if ok { std::env::current_dir().ok() } else { None });
        }
        #[cfg(target_arch = "x86_64")]
        unpack::init();
        patches::apply();
        wal_payload_common::amlog::init("SEGANU");
        wal_payload_common::jvs_board::init(devices::JVS_PORT);
        devices::init();
        lmaio::init();
        test_menu();
        log!("seganu: initialized");
    }
    TRUE
}

/// In the test menu program: the marker removed (`game.bat` deletes it after the test menu) and
/// the game's relaunch asked once the launcher is connected (the request is dropped before).
fn test_menu() {
    let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    if !exe.eq_ignore_ascii_case(devices::test_exe()) {
        return;
    }
    let _ = std::fs::remove_file(devices::test_marker());
    std::thread::spawn(|| {
        while !wal_payload_common::connected() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        log!("seganu: test menu: the game starts again when it ends");
        wal_payload_common::request_relaunch("");
    });
}
