//! Need for Speed Heat Takedown's seat shaker (a Buttkicker), off unless `WAL_NFS_BUTTKICKER=1`.
//!
//! The game plays a loud low rumble (`Buttkicker_Start`, 20-200 Hz, near full scale) for the
//! whole race, on its Wwise bus `MasterButt`, whose audio device `SystemButt` its
//! `SoundbankLoader` adds as a second output (`AkSoundEngine.AddOutput`, no device id: the
//! system's default device). The cabinet's own sound card wiring sends it to the shaker's amp;
//! on a PC it plays on the speakers, mixed with the game's sound. The output is not added
//! (`CSharp_AddOutput__SWIG_3`, `AkSoundEngine.dll`): the bus is not played. The music's low
//! end (`Music_LFE`, its `SystemLFE` output) is kept.

use std::sync::OnceLock;

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

/// The `SystemButt` audio device's shareset id (Wwise's FNV-1 hash of its lower-case name,
/// `Init.txt`).
const SYSTEM_BUTT: u32 = 2305542705;
const AK_SUCCESS: i32 = 1;

/// `WAL_NFS_BUTTKICKER=0`: the shaker's output dropped.
pub fn off() -> bool {
    static OFF: OnceLock<bool> = OnceLock::new();
    *OFF.get_or_init(|| std::env::var("WAL_NFS_BUTTKICKER").is_ok_and(|v| v.trim() == "0"))
}

pub fn resolve(name: &str) -> Option<usize> {
    (name == "CSharp_AddOutput__SWIG_3").then_some(add_output as *const () as usize)
}

/// `AddOutput(AkOutputSettings)`: `settings` the native `AkOutputSettings`, its shareset first.
unsafe extern "C" fn add_output(settings: *const u32) -> i32 {
    type F = unsafe extern "C" fn(*const u32) -> i32;
    static ORIG: OnceLock<usize> = OnceLock::new();
    if !settings.is_null() && unsafe { *settings } == SYSTEM_BUTT {
        log!("adrenaline: seat shaker output (SystemButt) not added (WAL_NFS_BUTTKICKER=0)");
        return AK_SUCCESS;
    }
    let orig = *ORIG.get_or_init(|| unsafe {
        let module = GetModuleHandleW(windows_sys::w!("AkSoundEngine.dll"));
        GetProcAddress(module, c"CSharp_AddOutput__SWIG_3".as_ptr().cast()).map_or(0, |f| f as usize)
    });
    if orig == 0 {
        return 2; // AK_Fail
    }
    unsafe { std::mem::transmute::<usize, F>(orig)(settings) }
}
