//! Sega Amusements Shell payload, loaded by `wal-loader`.
//!
//! * The Shell's TCP service (`shell.rs`): the game's events, the credits.
//! * The I/O board (`board.rs`): the game's board update replaced (`WAL_SEGASHELL_BOARD`).
//! * Profile code patches (`WAL_PATCHES`).
//! * The Shell's settings the game reads itself (`settings.rs`), overridden by the profile.
//! * Rapture3D's sound settings (`rapture3d.rs`).
//! * `WAL_SEGASHELL_PC_INPUT_HIDDEN=1`: the PC's keyboard, mouse and pads hidden from the game
//!   (`input.rs`).
//! * Force feedback on the parallel port (Showdown imports `inpout32.dll`'s `Out32`, ordinal
//!   2): its import made the payload's, the bytes decoded by payload-common's `port_ffb`
//!   (`WAL_SEGASHELL_FFB`, `WAL_SEGASHELL_FFB_FORMAT`).
//! * The game's resolution (`WAL_SEGASHELL_DEFAULT_RESOLUTION=<rva>`): its
//!   `DefaultResolution()` made to return 1280x720 (index 14) unless `WAL_SEGASHELL_FULLHD=1`
//!   (its own choice: 1920x1080).
//! * The test menu (`testmenu.rs`): TEST starts the Shell's program
//!   (`WAL_SEGASHELL_TEST_MENU`), which gets its own patches and inputs.

mod board;
mod input;
mod rapture3d;
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
        wal_payload_common::start("segashell");
        if testmenu::is_test_menu() {
            testmenu::init_shell();
            log!("segashell: test menu initialized");
            return TRUE;
        }
        patches::apply();
        resolution();
        settings::init();
        rapture3d::init();
        board::init();
        parallel_port();
        if std::env::var("WAL_SEGASHELL_PC_INPUT_HIDDEN").is_ok_and(|v| v.trim() == "1") {
            input::init();
        }
        shell::start();
        testmenu::watch_game();
        log!("segashell: initialized");
    }
    TRUE
}

/// The game's `inpout32.dll` imports (by ordinal: 1 `Inp32`, 2 `Out32`; its driver is not
/// there): the payload's.
fn parallel_port() {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    unsafe {
        wal_payload_common::iat::hook_ordinal(exe, "inpout32.dll", 1, inp32 as *const () as usize);
        wal_payload_common::iat::hook_ordinal(exe, "inpout32.dll", 2, out32 as *const () as usize);
    }
}

extern "system" fn inp32(_port: i16) -> i16 {
    0
}

extern "system" fn out32(port: i16, data: i16) {
    wal_payload_common::port_ffb::out("WAL_SEGASHELL", port as u16, data as u16);
}

/// `WAL_SEGASHELL_DEFAULT_RESOLUTION`: 1280x720 unless `WAL_SEGASHELL_FULLHD=1`.
fn resolution() {
    let Some(rva) = std::env::var("WAL_SEGASHELL_DEFAULT_RESOLUTION").ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
    else {
        return;
    };
    if std::env::var("WAL_SEGASHELL_FULLHD").is_ok_and(|v| v.trim() == "1") {
        log!("segashell: 1920x1080");
        return;
    }
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    // mov eax, 14 (1280x720); ret
    const CODE: [u8; 6] = [0xB8, 0x0E, 0x00, 0x00, 0x00, 0xC3];
    let at = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize + rva;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, CODE.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(CODE.as_ptr(), at as *mut u8, CODE.len());
        VirtualProtect(at as *const _, CODE.len(), old, &mut old);
    }
    log!("segashell: 1280x720");
}
