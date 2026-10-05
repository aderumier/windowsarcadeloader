//! Game code patches from the profile: `WAL_PATCHES=<rva>:<hex bytes>,...` written at
//! `image base + rva` of the game executable, e.g. `0xFF0A0:B801000000C3` (return true).
//! Keeps game-specific fixes out of the payload code.
//!
//! `WAL_PATCHES_EXE=<file name>` limits them to that executable: the payload is also loaded by
//! the other programs of the game directory (Tottemo E Mahjong's TestMode.exe was corrupted by
//! the patches of game2.exe).

use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

use crate::log;

pub fn apply() {
    let Ok(spec) = std::env::var("WAL_PATCHES") else { return };
    if let Ok(target) = std::env::var("WAL_PATCHES_EXE") {
        let exe = exe_name();
        if !exe.eq_ignore_ascii_case(target.trim()) {
            log!("patches: not applied to {exe} (WAL_PATCHES_EXE={target})");
            return;
        }
    }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((rva, hex)) = item.split_once(':') else {
            log!("patches: ignoring '{item}', expected rva:hexbytes");
            continue;
        };
        let rva = usize::from_str_radix(rva.trim_start_matches("0x"), 16);
        let bytes: Option<Vec<u8>> =
            (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
        let (Ok(rva), Some(bytes)) = (rva, bytes) else {
            log!("patches: ignoring malformed '{item}'");
            continue;
        };
        let addr = (base + rva) as *mut u8;
        let mut old = 0;
        unsafe {
            VirtualProtect(addr.cast(), bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr, bytes.len());
            VirtualProtect(addr.cast(), bytes.len(), old, &mut old);
        }
        log!("patches: {} bytes at +{rva:#x}", bytes.len());
    }
}

/// File name of the process executable.
fn exe_name() -> String {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_string()
}
