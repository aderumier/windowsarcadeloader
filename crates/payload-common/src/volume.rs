//! `WAL_VOLUME=<0..100>` (the common Volume option, default 100): the game's sound level in
//! percent. The game process's default audio session gets that volume (mmdevapi's
//! `ISimpleAudioVolume::SetMasterVolume`): Wine applies it to every stream of the session,
//! whatever API the game plays through (DirectSound and the DirectShow renderer on it, XAudio2,
//! WASAPI), before its mix leaves the process (After Dark (2.3)'s sounds and music, all at
//! full volume, saturated once summed).

use std::ffi::c_void;

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::log;

type P = *mut c_void;
type Guid = windows_sys::core::GUID;

const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_IMM_DEVICE_ENUMERATOR: Guid = Guid::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);
const IID_IAUDIO_SESSION_MANAGER: Guid = Guid::from_u128(0xbfa971f1_4d5e_40bb_935e_967039bfbee4);
const CLSCTX_ALL: u32 = 0x17;
const COINIT_MULTITHREADED: u32 = 0;
/// `eRender`, `eConsole`.
const RENDER: i32 = 0;
const CONSOLE: i32 = 0;

/// The `index`th method of a COM object's vtable.
unsafe fn method(object: P, index: usize) -> usize {
    unsafe { *(*(object as *const *const usize)).add(index) }
}

/// The default render device's default session (GUID_NULL, this process) at `level` (0..1).
/// The objects are kept (never released): the session's volume lives as long as the process.
unsafe fn set_session_volume(level: f32) -> Result<(), String> {
    let ole32 = unsafe { LoadLibraryW(windows_sys::w!("ole32.dll")) };
    let (Some(co_init), Some(co_create)) =
        (unsafe { GetProcAddress(ole32, c"CoInitializeEx".as_ptr().cast()) }, unsafe { GetProcAddress(ole32, c"CoCreateInstance".as_ptr().cast()) })
    else {
        return Err("ole32 not found".into());
    };
    let co_init: unsafe extern "system" fn(P, u32) -> i32 = unsafe { std::mem::transmute(co_init) };
    let co_create: unsafe extern "system" fn(*const Guid, P, u32, *const Guid, *mut P) -> i32 = unsafe { std::mem::transmute(co_create) };
    unsafe { co_init(std::ptr::null_mut(), COINIT_MULTITHREADED) };
    let check = |hr: i32, what: &str| if hr < 0 { Err(format!("{what} failed ({:#x})", hr as u32)) } else { Ok(()) };

    let mut enumerator: P = std::ptr::null_mut();
    check(unsafe { co_create(&CLSID_MM_DEVICE_ENUMERATOR, std::ptr::null_mut(), CLSCTX_ALL, &IID_IMM_DEVICE_ENUMERATOR, &mut enumerator) }, "MMDeviceEnumerator")?;
    // IMMDeviceEnumerator::GetDefaultAudioEndpoint
    let get_default: unsafe extern "system" fn(P, i32, i32, *mut P) -> i32 = unsafe { std::mem::transmute(method(enumerator, 4)) };
    let mut device: P = std::ptr::null_mut();
    check(unsafe { get_default(enumerator, RENDER, CONSOLE, &mut device) }, "GetDefaultAudioEndpoint")?;
    // IMMDevice::Activate
    let activate: unsafe extern "system" fn(P, *const Guid, u32, P, *mut P) -> i32 = unsafe { std::mem::transmute(method(device, 3)) };
    let mut manager: P = std::ptr::null_mut();
    check(unsafe { activate(device, &IID_IAUDIO_SESSION_MANAGER, CLSCTX_ALL, std::ptr::null_mut(), &mut manager) }, "Activate")?;
    // IAudioSessionManager::GetSimpleAudioVolume(GUID_NULL, not cross-process)
    let get_volume: unsafe extern "system" fn(P, *const Guid, u32, *mut P) -> i32 = unsafe { std::mem::transmute(method(manager, 4)) };
    let mut volume: P = std::ptr::null_mut();
    check(unsafe { get_volume(manager, std::ptr::null(), 0, &mut volume) }, "GetSimpleAudioVolume")?;
    // ISimpleAudioVolume::SetMasterVolume
    let set_master: unsafe extern "system" fn(P, f32, *const Guid) -> i32 = unsafe { std::mem::transmute(method(volume, 3)) };
    check(unsafe { set_master(volume, level, std::ptr::null()) }, "SetMasterVolume")
}

pub fn init() {
    let Some(percent) = std::env::var("WAL_VOLUME").ok().and_then(|v| v.trim().parse::<f32>().ok()) else { return };
    if percent >= 100.0 {
        return;
    }
    let level = (percent / 100.0).clamp(0.0, 1.0);
    // off the loader's thread (COM, mmdevapi's device list)
    std::thread::spawn(move || match unsafe { set_session_volume(level) } {
        Ok(()) => log!("volume: the game's audio at {percent}%"),
        Err(e) => log!("volume: not set: {e}"),
    });
}
