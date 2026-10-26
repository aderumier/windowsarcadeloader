//! Arcade PC payload: PC-based arcade games without a cabinet I/O API of their own to replace.
//!
//! Galaxy Rangers (`WXR.exe`, Unity 2019.4, Mono): the dump's `Assembly-CSharp.dll` is a
//! community build whose `ArcadeControls` class replaces the cabinet's camera guns and serial
//! I/O board with `Controls.ini`: each player aims with an XInput pad's left stick as an
//! absolute screen position (or a raw mouse), shoots with its right trigger, and the operator
//! menu is on the keyboard (`F2`, arrows). The payload is that `xinput1_3.dll` (the class's
//! `DllImport`):
//!
//! * `pads.rs`: four connected pads built from the virtual sticks: the gun position on the left
//!   stick, the trigger, reload, weapon change, start and coin buttons.
//! * `payload-common`'s `input_filter`: the real keyboard and mouse never reach the game
//!   (Unity's window messages and raw input, the class's `GetAsyncKeyState` polling); the
//!   operator keys it polls come from the virtual sticks.
//!
//! Unity loads `xinput1_4.dll` first: the profile disables it, so Unity loads this payload at
//! start too (its own XInput pads are ours) and the filters are in place before the first frame.

#![allow(non_snake_case)]

mod pads;

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

pub use pads::*;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("arcadepc");
        let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) as usize };
        wal_payload_common::input_filter::init(unity, pads::key_held);
    }
    1
}
