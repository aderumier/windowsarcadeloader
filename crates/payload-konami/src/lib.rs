//! Konami e-Amusement PC payload (Nobiteke! Byon Byon Daisakusen).
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

#![allow(non_snake_case)]

mod acio;
mod avs;
mod eamuse;
mod hbhi;
mod icca;
mod net;
mod screen;

use std::ffi::c_void;

use wal_payload_common::log;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use acio::*;
pub use hbhi::*;
pub use icca::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("konami");
        net::init();
        screen::init();
        avs::init();
        eamuse::init();
        log!("konami: ACIO board and card reader emulated (libacio.dll)");
    }
    TRUE
}
