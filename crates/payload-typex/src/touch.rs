//! Touch sensor driver of Block King Ball Shooter (`WAL_TYPEX_LSDRV=1`).
//!
//! The game reads its touch sensor through `lsdrv.dll` (a tablet driver: `OpenDriver`,
//! `TabletControl`...). Without the sensor `OpenDriver` fails (-1): "touch sensor error
//! (0xffffffff)" at boot. Its imports (by ordinal) are answered here as with a sensor present;
//! the touches themselves are written in the game's memory (`guns.rs`).

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

unsafe extern "system" fn ok0() -> i32 {
    0
}
unsafe extern "system" fn ok1(_a: usize) -> i32 {
    0
}
unsafe extern "system" fn ok2(_a: usize, _b: usize) -> i32 {
    0
}
unsafe extern "system" fn ok5(_a: usize, _b: usize, _c: usize, _d: usize, _e: usize) -> i32 {
    0
}
/// `GetTabletEnable`: the sensor is enabled.
unsafe extern "system" fn enabled() -> i32 {
    1
}

pub(crate) fn init() {
    if !std::env::var("WAL_TYPEX_LSDRV").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    // ordinal, function (stdcall, argument count from lsdrv.dll's `ret`)
    let stubs: [(u16, usize); 6] = [
        (2, ok0 as *const () as usize), // OpenDriver
        (3, ok0 as *const () as usize), // CloseDriver
        (6, ok2 as *const () as usize), // TabletMessage
        (7, ok1 as *const () as usize), // TabletControl
        (11, enabled as *const () as usize), // GetTabletEnable
        (36, ok5 as *const () as usize), // LsSetTabletParam
    ];
    for (ordinal, f) in stubs {
        unsafe { iat::hook_ordinal(base, "lsdrv.dll", ordinal, f) };
    }
    log!("touch: lsdrv.dll answered (sensor present)");
}
