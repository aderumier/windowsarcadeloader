//! Sega ALLS payload: the cabinet system the game and `amdaemon.exe` run on (Mario & Sonic at
//! the Tokyo 2020 Olympic Games Arcade, House of the Dead: Scarlet Dawn, Sega World Drivers
//! Championship).
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
//! - `WAL_ALLS_BOARD`: the game's controls (`io4.rs`; `hotd`: also the gun sensor board on
//!   COM1, `gunsensor.rs`, instead of the LED board; `swdc`: the force feedback board on COM1,
//!   `ffb.rs`, the LED board on COM2, the dashboard on COM20, `dashboard.rs`, the main
//!   cabinet's LAN announcements, `udp.rs`, the English translation, `english.rs`)
//! - `WAL_ALLS_AIME_PORT` (`COM3`...): the Aime card reader amdaemon opens there (`aime.rs`),
//!   `WAL_ALLS_ACCESS_CODE` the player's card
//! - `WAL_ALLS_DAEMON_PATCHES`: amdaemon's patches (`<rva>:<hex bytes>,...`)
//! - `WAL_PATCHES`: game code patches (same format)
//!
//! Network (`netenv.rs`, `allnet.rs`): one adapter in the keychip's subnet, the store router's
//! names resolved, ALL.Net answered by the payload's server.

mod aime;
mod aimedb;
mod allnet;
mod clock;
mod daemon;
mod dashboard;
mod english;
mod ffb;
mod gore;
mod gs;
mod gunsensor;
mod hotd;
mod icf;
mod io4;
mod led;
mod netenv;
mod pki;
mod registry;
mod sysfile;
mod udp;

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
            if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "hotd") {
                // COM1: the gun sensor board; COM22 the LED controller, absent
                gunsensor::init();
                hotd::init();
                gore::init();
                wal_payload_common::serial::install_sink("!COM22");
            } else if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "swdc") {
                // the main cabinet (StartApp.bat's marker, read by the game too: else it waits
                // for a main cabinet's announcement on the LAN)
                let _ = std::fs::create_dir_all("Y:\\SDDS");
                if !std::path::Path::new("Y:\\SDDS\\LanServer.dat").exists() {
                    let _ = std::fs::write("Y:\\SDDS\\LanServer.dat", b"");
                }
                // the main cabinet's announcements on the LAN, which it must hear itself
                udp::init();
                english::init();
                gs::init();
                // COM1: the force feedback board, COM2 the LED board, COM20 the dashboard
                ffb::init();
                led::init("COM2");
                dashboard::init();
            } else {
                led::init("COM1");
            }
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
