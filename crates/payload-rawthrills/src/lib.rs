//! Raw Thrills PC cabinet payload, loaded by `wal-loader` (the games read their I/O board
//! themselves, no driver DLL to replace).
//!
//! The cabinets have a JAMMA USB I/O board (a HID device: buttons, coins, wheel and pedals,
//! lamps) and a USB watchdog. The games open the board through SetupAPI/HID and parse its
//! reports in an I/O thread of their own.
//!
//! * `sdaemon.rs`: the JAMMA USB board games, The Fast and the Furious, its Drift and Super
//!   Bikes, Nicktoons Nitro Racing and H2O Overdrive (`WAL_RAWTHRILLS_GAME`): the board's
//!   reports built from the virtual sticks and handed to the game's report parser.
//! * `h2o.rs`: the `SetControl` games, The Fast and the Furious SuperCars, Super Alpine Racer,
//!   X-Games Snowboarder, Winter X Games SnoCross: the inputs through the game's `SetControl`.
//! * `sb2.rs`: The Fast and the Furious Super Bikes 2: the board inputs in the game's keyboard
//!   table (the dump reads them as keys), the analog values as its floats.
//! * `cars.rs`: Cars: its RIO board emulated behind its libusb0 imports.
//! * `dd.rs`: Dirty Drivin': its developer keyboard's floats and keys, the coin board's
//!   callback, the switch latches.
//! * `D:\` (logs, personal bests) redirected to `WindowsLoader` (`WAL_RAWTHRILLS_DDRIVE`).
//! * The cabinet reboot (`ExitWindowsEx`, H2O Overdrive leaving its service menu): the game is
//!   started again by the launcher instead, the process ended at once (its Direct3D device's
//!   teardown crashed in DXVK).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod cars;
mod common;
mod dd;
mod h2o;
mod sb2;
mod sdaemon;

use std::ffi::c_void;

use wal_payload_common::{drive, log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("rawthrills");
        patches::apply();
        drive::init("WAL_RAWTHRILLS_DDRIVE", "WindowsLoader");
        reboot_init();
        let game = std::env::var("WAL_RAWTHRILLS_GAME").unwrap_or_default();
        if !h2o::init(game.trim()) && !sb2::init(game.trim()) && !dd::init(game.trim()) && !cars::init(game.trim()) {
            sdaemon::init();
        }
        log!("rawthrills: initialized");
    }
    TRUE
}

/// `ExitWindowsEx` (the cabinet reboot): the launcher starts the game again, the process ends
/// at once.
unsafe extern "system" fn exit_windows_ex(flags: u32, _reason: u32) -> i32 {
    log!("rawthrills: the game reboots the cabinet (ExitWindowsEx {flags:#x}): started again");
    wal_payload_common::request_relaunch("");
    // the request reaches the launcher before the connection is torn down
    std::thread::sleep(std::time::Duration::from_millis(300));
    unsafe {
        windows_sys::Win32::System::Threading::TerminateProcess(windows_sys::Win32::System::Threading::GetCurrentProcess(), 0)
    };
    1
}

static EXIT_PROCESS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `ExitProcess`: logged with the game addresses on the stack (who ends the game).
unsafe extern "system" fn exit_process(code: u32) {
    let mut sp = 0usize;
    let at = &mut sp as *mut usize as usize;
    let mut callers = Vec::new();
    for i in 0..256 {
        let v = unsafe { ((at + i * 4) as *const usize).read_volatile() };
        if (0x401000..0x700000).contains(&v) {
            callers.push(format!("{v:#x}"));
        }
        if callers.len() >= 12 {
            break;
        }
    }
    log!("rawthrills: ExitProcess({code}) from {}", callers.join(" "));
    let orig: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(EXIT_PROCESS.load(std::sync::atomic::Ordering::Relaxed)) };
    unsafe { orig(code) }
}

fn reboot_init() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "ExitProcess", exit_process as *const () as usize) } {
        EXIT_PROCESS.store(o, std::sync::atomic::Ordering::Relaxed);
    }
    if unsafe { wal_payload_common::iat::hook("user32.dll", "ExitWindowsEx", exit_windows_ex as *const () as usize) }.is_some() {
        log!("rawthrills: cabinet reboot made a relaunch");
    }
}
