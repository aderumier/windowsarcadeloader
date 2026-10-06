//! Tsunami (TsuMo Racing) cabinet payload, loaded by `wal-loader` (Re-Volt imports no cabinet
//! driver DLL: plain DirectInput, DirectDraw, Miles sound).
//!
//! On the cabinet, `tsuinput.exe` reads the wheel, pedals, buttons and coin mechanism and
//! exposes them as the `TsuInput` COM object (`TsuInputLib.dll`). This payload emulates it
//! from the virtual arcade sticks.
//!
//! * `dinput.rs`: Wine's DirectInput (keyboard/mouse) with the host joysticks hidden.
//! * `tsuinput.rs`: the TsuInput COM object (wheel, pedals, cabinet buttons, credits).
//! * `tsumotion.rs`: the TsuMotion COM object (motion seat), an idle seat.
//! * `tsunet.rs`: registers the dump's `tsunet.dll` (the second cabinet COM object) with Wine.
//!
//! The payload is itself an in-proc COM server (`DllGetClassObject`): the TsuInput object is
//! registered under this DLL, so the game gets it through Wine's COM even if the
//! `CoCreateInstance` IAT hook is lost.

#![allow(non_snake_case)]

mod dinput;
mod tsuinput;
mod tsumotion;
mod tsunet;

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameA;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

static PAYLOAD_MODULE: AtomicUsize = AtomicUsize::new(0);

/// Full path of this payload DLL (its in-proc COM registration points to it).
pub(crate) fn dll_path() -> Option<String> {
    let module = PAYLOAD_MODULE.load(Ordering::Relaxed) as HINSTANCE;
    if module.is_null() {
        return None;
    }
    let mut buf = [0u8; 260];
    let n = unsafe { GetModuleFileNameA(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf8_lossy(&buf[..n]).into_owned())
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        PAYLOAD_MODULE.store(module as usize, Ordering::Relaxed);
        wal_payload_common::start("tsunami");
        dinput::init();
        tsunet::init();
        tsumotion::init();
        tsuinput::init();
        log!("tsunami: initialized");
    }
    TRUE
}

/// COM in-proc server entry point: the TsuInput object (`tsuinput.rs`).
#[unsafe(no_mangle)]
pub extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, out: *mut *mut c_void) -> i32 {
    unsafe { tsuinput::dll_get_class_object(rclsid, riid, out) }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> i32 {
    1 // S_FALSE: the payload stays loaded
}
