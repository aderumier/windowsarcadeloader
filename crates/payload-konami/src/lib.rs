//! Konami e-Amusement PC payload (Nobiteke! Byon Byon Daisakusen, Gashaaaan Refill!).
//!
//! The game reaches its cabinet I/O through `libacio.dll`, Konami's ACIO serial bus driver:
//! the HBHI I/O board (switches, two controllers of three analog axes each, coin slots, coin
//! counters, lamps, the security plugs) and the ICCA e-Amusement card reader. This DLL takes its
//! place and answers the functions the game imports, fed by the virtual arcade sticks.
//!
//! * `hbhi.rs`: the I/O board status buffer, its security plugs, coin and lamp outputs.
//! * `icca.rs`: the card reader (the `card` control inserts / ejects a card).
//! * `acio.rs`: the bus itself (start, devices present, versions).
//! * `net.rs`: the cabinet's own network address setup, answered.
//! * `eamuse.rs`: e-Amusement offline (the boot's connection check).
//! * `screen.rs`: the picture at the screen's resolution, a borderless window.
//! * `avs.rs`: the AVS log, on request.
//!
//! Otomedius loads it as `libacio.dll` too (`pjei.rs`, the PJEI board), its libbms and
//! libtouchpanel imports pointed at the payload (`otomedius.rs`, `touchpanel.rs`).
//!
//! Gashaaaan Refill! loads it as `libbms.dll` (its BM-System client, `bms.rs`), from where
//! `ig2.rs` answers the game's own I/O library, ig2ACLib; `crosshair.rs` shows the players' aim.

#![allow(non_snake_case)]

mod acio;
mod avs;
mod bms;
mod crosshair;
mod eamuse;
mod hbhi;
mod icca;
mod ig2;
mod net;
mod otomedius;
mod pjei;
mod screen;
mod touchpanel;

use std::ffi::c_void;

use wal_payload_common::log;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use acio::*;
pub use bms::*;
pub use hbhi::*;
pub use icca::*;
pub use pjei::*;
pub use touchpanel::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("konami");
        if file_name(module).eq_ignore_ascii_case("libbms.dll") {
            ig2::init();
            crosshair::init();
            return TRUE;
        }
        if file_name(std::ptr::null_mut()).eq_ignore_ascii_case("otomedius.exe") {
            otomedius::init(module);
            log!("konami: PJEI board, touch screen and BM-System emulated (Otomedius)");
            return TRUE;
        }
        net::init();
        screen::init();
        avs::init();
        eamuse::init();
        log!("konami: ACIO board and card reader emulated (libacio.dll)");
    }
    TRUE
}

/// This DLL's file name (what it was installed as), or the executable's for a null module.
fn file_name(module: HINSTANCE) -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_string()
}
