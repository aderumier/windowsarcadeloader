//! Video for Windows codecs shipped with a game (`WAL_VFW_CODECS`).
//!
//! `WAL_VFW_CODECS=vidc.wmv3=WMV9VCM.dll[,...]` registers each driver in
//! `HKLM\Software\Microsoft\Windows NT\CurrentVersion\Drivers32` (what a codec installer does),
//! where Wine's `ICOpen` looks it up; the DLL is loaded by name, from the game directory.
//! Gaia Attack 4 plays WMV9 (`WMV3`) AVIs through AVIFile and ships Microsoft's WMV9 VCM
//! codec installer: its `WMV9VCM.dll` is self-contained.

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegSetValueExW,
};

use crate::log;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Registers the codecs of `WAL_VFW_CODECS`, if any.
pub fn init() {
    let Ok(spec) = std::env::var("WAL_VFW_CODECS") else { return };
    let path = wide("Software\\Microsoft\\Windows NT\\CurrentVersion\\Drivers32");
    let mut key: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut())
    };
    if r != 0 {
        log!("vfw: cannot open Drivers32: error {r}");
        return;
    }
    for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((name, dll)) = entry.split_once('=') else {
            log!("vfw: bad entry {entry:?}");
            continue;
        };
        let (name, dll) = (wide(name.trim()), wide(dll.trim()));
        let r = unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, dll.as_ptr().cast(), (dll.len() * 2) as u32) };
        log!("vfw: {entry} {}", if r == 0 { "registered".to_string() } else { format!("error {r}") });
    }
    unsafe { RegCloseKey(key) };
}
