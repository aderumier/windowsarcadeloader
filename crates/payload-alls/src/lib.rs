//! Sega ALLS payload: the cabinet system the game and `amdaemon.exe` run on (Mario & Sonic at
//! the Tokyo 2020 Olympic Games Arcade).
//!
//! An ALLS game talks to `amdaemon.exe` (keychip, ALL.Net, the USB I/O board, credits), which
//! talks to the hardware. Both programs are started by `wal-loader` with this payload (the
//! profile's `background` list starts amdaemon first). In amdaemon (`daemon.rs`) it writes
//! the cabinet's system registry values, emulates the keychip driver, the hardware monitor and
//! the "Type 4" USB I/O board (`io4.rs`, fed by the launcher's sticks). In the game it filters
//! the real keyboard and pads and answers the LED board on COM1 (`led.rs`).
//!
//! Payload options (profile `env`):
//! - `WAL_ALLS_GAME_ID` (4 letters), `WAL_ALLS_PLATFORM_ID` (default `ACA1`),
//!   `WAL_ALLS_KEYCHIP` (default `A69E-01A88888888`), `WAL_ALLS_REGION` (1 Japan, 4 export,
//!   8 China; default 1), `WAL_ALLS_SUBNET` (default `192.168.149.0`): the keychip's values
//! - `WAL_ALLS_DIPSW`: the system DIP switches 1-8 (bits 0-7, hex; default 01: cabinet 1),
//!   `WAL_ALLS_FREEPLAY` (else the common `WAL_FREEPLAY`, default on): in the system backup
//!   (`sysfile.rs`, from amdaemon's second start)
//! - `WAL_ALLS_ALLNET=1`: ALL.Net authentication accepted (`allnet.rs`; default refused: the game
//!   plays offline)
//! - `WAL_ALLS_CLOCK=0`: the real time zone (default Japan's, `clock.rs`)
//! - `WAL_ALLS_DAEMON_PATCHES`: amdaemon's patches (`<rva>:<hex bytes>,...`)
//! - `WAL_PATCHES`: game code patches (same format)
//!
//! Network (`netenv.rs`, `allnet.rs`): one adapter in the keychip's subnet, the store router's
//! names resolved, ALL.Net answered by the payload's server.

mod allnet;
mod clock;
mod daemon;
mod io4;
mod led;
mod netenv;
mod pki;
mod registry;
mod sysfile;

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

static mut MODULE: HINSTANCE = std::ptr::null_mut();

/// This payload's own module (amdaemon loads its video library from it: `amvideo`).
pub(crate) fn module() -> HINSTANCE {
    unsafe { MODULE }
}

fn is_daemon() -> bool {
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())).is_some_and(|n| n == "amdaemon.exe")
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { MODULE = module };
        wal_payload_common::start("alls");
        if std::env::var("WAL_ALLS_CLOCK").map_or(true, |v| v.trim() != "0") { clock::init(); }
        if is_daemon() {
            daemon::init();
        } else {
            let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
            // the cabinet's input comes from amdaemon's I/O board: the real keyboard and pads
            // never reach the game
            wal_payload_common::input_filter::init(exe, |_| false);
            wal_payload_common::patches::apply();
            led::init();
            netenv::init_game();
            allnet::init_redirect();
        }
    }
    1
}

// amvideo: the cabinet's video driver library amdaemon loads (registry `amVideo\name`): the
// screen mode is the launcher's

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoOpen(_ctx: *mut c_void) -> i32 {
    wal_payload_common::log!("alls: amDllVideoOpen");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoClose(_ctx: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoSetResolution(_ctx: *mut c_void, _param: *mut c_void) -> i32 {
    wal_payload_common::log!("alls: amDllVideoSetResolution");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn amDllVideoGetVBiosVersion(_ctx: *mut c_void, dest: *mut u8, size: usize) -> i32 {
    let version = b"01.02.03.04.05\0";
    if !dest.is_null() && size >= version.len() {
        unsafe { std::ptr::copy_nonoverlapping(version.as_ptr(), dest, version.len()) };
    }
    0
}
