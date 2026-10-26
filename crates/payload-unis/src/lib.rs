//! UNIS payload: the cabinet hardware of UNIS's PC-based arcade games.
//!
//! Dino Invasion (Unity 5.4, Mono, 64-bit; `WAL_UNIS_BOARD=shunxiang`, the default):
//! `shunxiang.rs` emulates its COM3 I/O board, the guns' camera tracker (`LC64.dll`) and its
//! dongles, through Mono's `DllImport`s (`payload-common`'s `mono_imports.rs`).
//!
//! The payload is `xinput1_3.dll`, which Unity loads at start: no pads (the game's input comes
//! from the emulated hardware), the real keyboard and mouse filtered (`payload-common`'s
//! `input_filter`), the common crosshair option over the Direct3D 11 picture.

mod shunxiang;

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("unis");
        // the common crosshair option: player 1's gun on the picture (Direct3D 11)
        if wal_payload_common::crosshairs(false) {
            wal_payload_common::pointer_d3d11::init(wal_payload_common::pointer_d3d11::Dot {
                color: [1.0, 0.15, 0.15],
                size: 0.06,
                position: wal_payload_common::gun::gun_in_picture,
                crosshair: true,
            });
        }
        // Unity 5.4: the player is the executable (no UnityPlayer.dll)
        let mut unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
        if unity == 0 {
            unity = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        }
        wal_payload_common::input_filter::init(unity, |_| false);
        match std::env::var("WAL_UNIS_BOARD").unwrap_or_default().trim() {
            "" | "shunxiang" => shunxiang::init(),
            other => wal_payload_common::log!("unis: unknown board {other}"),
        }
    }
    1
}

// XInput: no pads, the cabinet's inputs are the emulated hardware's

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetState(_index: u32, _state: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetStateEx(_index: u32, _state: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputSetState(_index: u32, _vibration: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetCapabilities(_index: u32, _flags: u32, _caps: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputEnable(_enable: i32) {}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetDSoundAudioDeviceGuids(_index: u32, _render: *mut c_void, _capture: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetBatteryInformation(_index: u32, _kind: u8, _info: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetKeystroke(_index: u32, _reserved: u32, _keystroke: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}
