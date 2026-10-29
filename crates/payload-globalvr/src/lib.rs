//! Global VR payload (Far Cry Paradise Lost, The Swarm, Aliens Extermination, Need for Speed GT, Underground, Carbon,
//! NASCAR Team Racing).
//!
//! Far Cry imports `USBIOExtreme.dll` (driver of Global VR's USBIO board: gun positions,
//! gun and panel buttons, coin counters) statically: this DLL takes its place in the run
//! directory, so it loads before the game code without any injector. The driving cabinets
//! import it as `GVRInputRaw.dll` (both of its exports sets are in `USBIOExtreme.def`).
//!
//! * `usbio.rs`: the `CUSBIO` class the game calls, fed by the virtual arcade sticks.
//! * `W:\` redirected to the game directory (the cabinet runs the game from `subst W: .`):
//!   `WAL_GLOBALVR_WDRIVE`, default `.`.
//! * `g_arcadeError` of `systemcfg.lua` reset at load (`WAL_GLOBALVR_CLEAR_ERRORS=0` keeps it).
//! * `hasp.rs`: the game's HASP4 dongle calls.
//! * `gvrinput.rs`: `GVRInputRaw.dll` of the driving cabinets (Need for Speed GT), the wheel,
//!   pedals and buttons of the frontend and of the game it starts; `frontend_fit.rs`: Need for
//!   Speed Underground's .NET frontend window fitted to the screen.
//! * `gvrio.rs`: `GvrIO.dll` of the "Zeus" driving cabinets (Need for Speed Carbon), its messages.
//! * `carbon_menu.rs`: Need for Speed Carbon's race selection screen (`WAL_GLOBALVR_CARBON_MENU=1`).
//! * `carbon_shell.rs`: the cabinet monitors Need for Speed Carbon's real frontend needs.
//! * `linking.rs`: the NASCAR frontends' `GvrLinking.dll` (a cabinet alone).
//! * `netcomm.rs`: NASCAR Team Racing's `gvrNetComm.dll` (no cabinet linked).
//! * `aklog.rs`: the Anark player's logger (its messages and script errors), for debugging.
//! * `beachhead.rs`: Beach Head 2000's frontend (`UniverShell.exe`) and game (`Beach16.exe`),
//!   `beachhead_io.rs` their cabinet controls, `amovie.rs` the ActiveMovie control of the
//!   frontend's HTML pages (their attract movies).
//!   Beach Head 2002's and Desert War's multigame shell and games (`BH2.exe`, `desertwar.exe`)
//!   the same way, their controls through the older `GVRInputRaw.dll` (`turret.rs`).
//! * `blazing.rs`: Blazing Angels' coin and volume monitors, dongle and player profile.
//! * `smartcard.rs`: Need for Speed Carbon's frontend card reader (`GVRSCR28.dll`).
//! * `aliens.rs`: Aliens Extermination's guns and panel (`WAL_GLOBALVR_ALIENS=1`).
//! * `keyboard.rs`: the game's own keyboard reduced to the operator keys (test menu).
//! * `swarm.rs`: The Swarm's input adapter (shared memory), `WAL_GLOBALVR_SWARM=1`, and its
//!   stage changes (`swarm_campaign.rs`: the game started again with the next map) and the
//!   imports its dump left unresolved (`swarm_imports.rs`).
//! * `gamelog.rs`: the game's own messages in the payload log (`WAL_GLOBALVR_GAMELOG=1`).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod aklog;
mod aliens;
mod amovie;
mod beachhead;
mod beachhead_io;
mod beachhead_save;
mod blazing;
mod carbon_menu;
mod carbon_shell;
mod config;
mod frontend_fit;
mod gamelog;
mod gvrinput;
mod gvrio;
mod hasp;
mod keyboard;
mod linking;
mod netcomm;
mod smartcard;
mod swarm;
mod swarm_campaign;
mod swarm_imports;
mod turret;
mod usbio;

use std::ffi::c_void;

use wal_payload_common::{drive, log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use gvrinput::*;
pub use gvrio::*;
pub use linking::*;
pub use netcomm::*;
pub use smartcard::*;
pub use usbio::*;

/// The process executable's image size (`SizeOfImage`): the fixed game offsets the modules
/// check must lie inside it (the payload also loads in the other programs of a game).
pub(crate) fn exe_size() -> usize {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    unsafe {
        let nt = base + ((base + 0x3c) as *const u32).read_unaligned() as usize;
        ((nt + 0x50) as *const u32).read_unaligned() as usize
    }
}

/// The file name of a loaded module.
fn module_name(module: HINSTANCE) -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(module, buf.as_mut_ptr(), 260) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit('\\').next().unwrap_or(&path).to_string()
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // the copies installed as gvrNetComm.dll / GvrLinking.dll: their exports only (the
        // GvrIO.dll copy initializes)
        let name = module_name(module).to_ascii_lowercase();
        if name == "gvrnetcomm.dll" || name == "gvrlinking.dll" {
            return TRUE;
        }
        wal_payload_common::start("globalvr");
        patches::apply();
        drive::init_letter(b'W', "WAL_GLOBALVR_WDRIVE", ".");
        if std::env::var("WAL_GLOBALVR_CLEAR_ERRORS").map_or(true, |v| v != "0") {
            config::clear_errors();
        }
        usbio::init();
        gvrinput::init(module);
        gvrio::init();
        carbon_menu::init();
        carbon_shell::init();
        smartcard::init();
        aliens::init();
        hasp::init();
        keyboard::init();
        gamelog::init();
        swarm::init();
        beachhead::init(module);
        blazing::init();
        log!("globalvr: initialized");
    }
    TRUE
}
