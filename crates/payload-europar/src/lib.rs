//! Sega Europa-R payload (Ford Racing Full Blown).
//!
//! The game imports `inpout32.dll` (parallel port access, the force feedback board) statically:
//! this DLL takes its place in the run directory, so it loads before the game code without any
//! injector.
//!
//! * `happ.rs`: the Happ UGCI USB board (HID: wheel, pedals, buttons), found through SetupAPI
//!   and read with `ReadFile`, emulated from the virtual sticks.
//! * `shell.rs`: the cabinet's GameShell (coins, credits): its window, the game's
//!   `WM_COPYDATA` messages answered.
//! * `ffb.rs`: force feedback, the bytes the game writes to the parallel port (`Out32`).
//! * `settings.rs`: operator settings (resolution, free play) from profile options.
//! * `testmenu.rs`: TEST runs the GameShell's test menu (the payload is loaded there too).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod ffb;
mod happ;
mod settings;
mod shell;
mod testmenu;

use std::ffi::c_void;

use wal_payload_common::{log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("europar");
        if testmenu::is_test_menu() {
            // the GameShell's test menu (loaded by wal-loader)
            happ::init(true);
            testmenu::init_shell();
            log!("europar: initialized in the test menu");
        } else {
            patches::apply();
            settings::init();
            happ::init(false);
            shell::init();
            testmenu::watch_game();
            log!("europar: initialized");
        }
    }
    TRUE
}

/// `short Inp32(short port)`: nothing is read from the parallel port.
#[unsafe(no_mangle)]
pub extern "system" fn inp32(port: i16) -> i16 {
    log!("europar: Inp32({port:#x})");
    0
}

/// `void Out32(short port, short data)`: the force feedback board on the parallel port.
#[unsafe(no_mangle)]
pub extern "system" fn out32(port: i16, data: i16) {
    ffb::out(port as u16, data as u16);
}
