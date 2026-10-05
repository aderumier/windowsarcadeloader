//! Namco ES3 payload (Star Wars Battle Pod: The Force Awakens, 64-bit).
//!
//! It takes the place of `hasp_windows_x64_100610.dll`, imported (by ordinal) by both the
//! cabinet launcher `Launcher\RSLauncher.exe` and the game it starts,
//! `Binaries\Win64\SWArcGame-Win64-Shipping.exe`: loaded in both processes without injector.
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

mod dinput;
mod hasp;
mod jvs;
mod patches;

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

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("namcoes3");
        let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let exe = exe.unwrap_or_default();
        let base = module_base();
        if exe.eq_ignore_ascii_case(LAUNCHER) {
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
            if std::env::var("WAL_NAMCOES3_GAME_XINPUT").map_or(true, |v| v != "1") {
                unsafe {
                    iat::hook_ordinal(base, "xinput1_3.dll", 2, xinput_get_state as *const () as usize);
                    iat::hook_ordinal(base, "xinput1_3.dll", 3, xinput_set_state as *const () as usize);
                }
                dinput::init();
            }
            patches::game(base);
        } else {
            log!("namcoes3: loaded in {exe}, nothing to do");
        }
        log!("namcoes3: initialized in {exe}");
    }
    TRUE
}

fn module_base() -> usize {
    unsafe { GetModuleHandleW(std::ptr::null()) as usize }
}
