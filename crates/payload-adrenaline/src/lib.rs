//! Adrenaline Amusements payload: the cabinet hardware of Adrenaline's PC-based arcade games.
//!
//! Skylanders Cloud Patrol (Unity 4.2, Mono `mono.dll`, 32-bit): a touch screen game.
//! `pqmt.rs` is its PQ Labs touch frame's client (`PQMTClient.dll`: the players' guns touch the
//! screen), `kaboom.rs` its "Kaboom" I/O board (`KaboomComDll.dll`: coins, the operator's
//! keypad, the ticket dispenser) and its Matrix dongle (`matrix32.dll`), all through Mono's
//! `DllImport`s (`payload-common`'s `mono_imports.rs`).
//!
//! The payload is `xinput1_3.dll`, which Unity loads at start: no pads (the game's input comes
//! from the emulated hardware), the real keyboard and mouse filtered (`payload-common`'s
//! `input_filter`: the game's own `C` coin and `O` operator keys), the common crosshair option
//! over the Direct3D 9 picture.

mod kaboom;
mod pqmt;

use std::ffi::c_void;
use std::sync::OnceLock;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, mono_imports};
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// The players: one finger (touch point) each.
const PLAYERS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// The player's finger on the screen, where the gun points.
    Touch,
    Coin,
    /// The operator's keypad (any key opens and closes the operator menu).
    Operator,
}

const NATIVES: &[(&str, Native)] = &[("touch", Native::Touch), ("coin", Native::Coin), ("operator", Native::Operator)];

const DEFAULTS: &[(&str, &str)] = &[("b1", "touch"), ("coin", "coin"), ("test", "operator"), ("service", "operator")];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn held(player: usize, native: Native) -> bool {
    let mut on = false;
    map().for_each_pressed(&input(player), |n| on |= n == native);
    on
}

/// The emulated hardware's functions.
fn resolve(name: &str, module: &str) -> Option<usize> {
    match module {
        "kaboomcomdll.dll" | "matrix32.dll" => kaboom::resolve(name),
        "pqmtclient.dll" => pqmt::resolve(name),
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("adrenaline");
        // the common crosshair option: where each gun points (Direct3D 9)
        wal_payload_common::crosshair::init();
        // Unity 4.2: the player is the executable
        let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        wal_payload_common::input_filter::init(exe, |_| false);
        mono_imports::hook(resolve);
        kaboom::init();
        pqmt::init();
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
