//! Namco ES3 payload (64-bit): Star Wars Battle Pod: The Force Awakens, Lost Land Adventure,
//! Wangan Midnight Maximum Tune 5..6RR, Time Crisis 5, Tekken 7, Pokkén Tournament.
//!
//! It takes the place of the HASP HL library both the cabinet launcher `RSLauncher.exe` and
//! the game it starts import: loaded in both processes without injector. The library's name
//! tells the game: `hasp_windows_x64_100610.dll` Battle Pod (`Launcher\RSLauncher.exe`,
//! `Binaries\Win64\SWArcGame-Win64-Shipping.exe`), `hasp_windows_x64_106993.dll` Lost Land
//! Adventure (`RSLauncher.exe`, `LLA\Binaries\Win64\DomeShooterGame-Win64-Shipping.exe`,
//! `lla.rs`), `hasp_windows_x64_109906.dll` Wangan Midnight Maximum Tune 5,
//! `hasp_windows_x64_98199.dll` 5DX and `hasp_windows_x64_106482.dll` 5DX+ (`wmn5r.exe`,
//! `wmmt5.rs`), `hasp_windows_x64_105293.dll` Time Crisis 5 (`RSLauncher.exe`,
//! `TC5\Binaries\Win64\TimeCrisisGame-Win64-Shipping.exe`, `tc5.rs`, USIO board `usio.rs`).
//! Tekken 7 and Pokkén Tournament import no cabinet library: the payload is their
//! `xinput1_3.dll` (`tekken7.rs`, `pokken.rs`).
//!
//! * `hasp.rs`: the HASP HL dongle API.
//! * `jvs.rs`: the JVS I/O board, on the serial calls of the launcher and of the game's I/O
//!   libraries (`wajvio.dll`, `wajvio_com.dll`).
//! * `patches.rs`: projector check, flat screen, language, USB key.
//! * The game also reads XInput and DirectInput pads directly (developer controls): it gets
//!   neutral XInput pads and no DirectInput controller (`WAL_NAMCOES3_GAME_XINPUT=1` keeps
//!   them).
//! * `dinput.rs`: DirectInput device enumeration filter.

#![allow(non_snake_case)]

mod amcus;
mod banapass;
mod bngrw;
mod bpreader;
mod dinput;
mod hasp;
mod jvs;
mod lla;
mod patches;
mod pokken;
mod projector;
mod tc5;
mod tekken7;
mod usio;
mod wmmt5;
mod wmmt5_net;
mod wmmt5_terminal;

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use hasp::*;

/// Image base of the game executable (0 in the launcher process).
pub(crate) static GAME_BASE: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn game_base() -> usize {
    GAME_BASE.load(Ordering::Relaxed)
}

const LAUNCHER: &str = "RSLauncher.exe";
const GAME: &str = "SWArcGame-Win64-Shipping.exe";

fn module(name: &str) -> usize {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe { GetModuleHandleW(wide.as_ptr()) as usize }
}

/// XINPUT_STATE: packet number, then the pad state.
unsafe extern "system" fn xinput_get_state(_index: u32, state: *mut u8) -> u32 {
    if !state.is_null() {
        unsafe { std::ptr::write_bytes(state, 0, 16) };
    }
    0
}

unsafe extern "system" fn xinput_set_state(_index: u32, _vibration: *mut c_void) -> u32 {
    0
}

/// The game's own XInput and DirectInput pads (developer controls): neutral XInput pads, no
/// DirectInput controller (`WAL_NAMCOES3_GAME_XINPUT=1` keeps them).
pub(crate) fn game_pads_hidden(base: usize) {
    if std::env::var("WAL_NAMCOES3_GAME_XINPUT").is_ok_and(|v| v == "1") {
        return;
    }
    unsafe {
        iat::hook_ordinal(base, "xinput1_3.dll", 2, xinput_get_state as *const () as usize);
        iat::hook_ordinal(base, "xinput1_3.dll", 3, xinput_set_state as *const () as usize);
    }
    dinput::init();
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("namcoes3");
        let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let exe = exe.unwrap_or_default();
        let base = module_base();
        if exe.eq_ignore_ascii_case(pokken::GAME) {
            pokken::attach(base);
        } else if tekken7::is_tekken7(&exe) {
            tekken7::attach(&exe, base);
        } else if tc5::is_tc5() {
            tc5::attach(&exe, base);
        } else if lla::is_lla() {
            lla::attach(&exe, base);
        } else if let Some(version) = wmmt5::version() {
            wmmt5::attach(&exe, base, version);
        } else if exe.eq_ignore_ascii_case(LAUNCHER) {
            jvs::init(&[base]);
            patches::launcher(base);
        } else if exe.eq_ignore_ascii_case(GAME) {
            GAME_BASE.store(base, Ordering::Relaxed);
            // the game opens the board through wajvio.dll; wajvio_com.dll is the same library
            let libs: Vec<usize> = ["wajvio.dll", "wajvio_com.dll"].into_iter().map(module).filter(|m| *m != 0).collect();
            if libs.is_empty() {
                log!("namcoes3: wajvio.dll not loaded, no I/O board");
            }
            jvs::init(&libs);
            game_pads_hidden(base);
            patches::game(base);
        } else {
            log!("namcoes3: loaded in {exe}, nothing to do");
        }
        log!("namcoes3: initialized in {exe}");
    }
    TRUE
}

/// The PE time stamp of the module loaded at `base`.
pub(crate) fn exe_timestamp(base: usize) -> u32 {
    unsafe {
        let pe = base + *((base + 0x3c) as *const u32) as usize;
        *((pe + 8) as *const u32)
    }
}

pub(crate) fn module_base() -> usize {
    unsafe { GetModuleHandleW(std::ptr::null()) as usize }
}
