//! Sega Amusements Shell payload, loaded by `wal-loader`.
//!
//! * The Shell's TCP service (`shell.rs`): the game's events, the credits.
//! * The I/O board (`board.rs`): the game's board update replaced (`WAL_SEGASHELL_BOARD`).
//! * Profile code patches (`WAL_PATCHES`).
//! * The game's resolution (`WAL_SEGASHELL_DEFAULT_RESOLUTION=<rva>`): its
//!   `DefaultResolution()` made to return 1280x720 (index 14) unless `WAL_SEGASHELL_FULLHD=1`
//!   (its own choice: 1920x1080).

mod board;
mod shell;

use std::ffi::c_void;

use wal_payload_common::{log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("segashell");
        patches::apply();
        resolution();
        board::init();
        shell::start();
        log!("segashell: initialized");
    }
    TRUE
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
