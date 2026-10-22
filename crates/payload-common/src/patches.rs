//! Game code patches from the profile: `WAL_PATCHES=<rva>:<hex bytes>,...` written at
//! `image base + rva` of the game executable, e.g. `0xFF0A0:B801000000C3` (return true).
//! Keeps game-specific fixes out of the payload code.
//!
//! `WAL_PATCHES_EXE=<file name>` limits them to that executable: the payload is also loaded by
//! the other programs of the game directory (Tottemo E Mahjong's TestMode.exe was corrupted by
//! the patches of game2.exe).
//!
//! `<module>!<rva>:<hex bytes>` patches a DLL the game loads later (an Unreal plugin's
//! library): applied by a thread as soon as the module is loaded (checked every 50 ms, 10 min).

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
    apply_list(&spec);
}

/// Applies a patch list in the `WAL_PATCHES` format to the process executable (and the
/// `<module>!` ones to their DLLs), whatever `WAL_PATCHES_EXE` says: for payloads with their
/// own patch lists per executable (the Sega Shell's test menu program).
pub fn apply_list(spec: &str) {
    let base = crate::iat::game_module();
    let mut later: Vec<(String, String)> = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((module, patch)) = item.split_once('!') {
            later.push((module.to_string(), patch.to_string()));
            continue;
        }
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
        unsafe { write(base + rva, &bytes) };
        log!("patches: {} bytes at +{rva:#x}", bytes.len());
    }
    if !later.is_empty() {
        std::thread::spawn(move || patch_modules(later));
    }
}

unsafe fn write(at: usize, bytes: &[u8]) {
    let addr = at as *mut u8;
    let mut old = 0;
    unsafe {
        VirtualProtect(addr.cast(), bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr, bytes.len());
        VirtualProtect(addr.cast(), bytes.len(), old, &mut old);
    }
}

/// Patches of modules loaded after the payload, each applied once its module is there.
fn patch_modules(mut pending: Vec<(String, String)>) {
    for _ in 0..12000 {
        pending.retain(|(module, patch)| {
            let wide: Vec<u16> = module.encode_utf16().chain(Some(0)).collect();
            let base = unsafe { GetModuleHandleW(wide.as_ptr()) } as usize;
            if base == 0 {
                return true;
            }
            let parsed = patch.split_once(':').and_then(|(rva, hex)| {
                let rva = usize::from_str_radix(rva.trim_start_matches("0x"), 16).ok()?;
                let bytes: Option<Vec<u8>> =
                    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
                Some((rva, bytes?))
            });
            match parsed {
                Some((rva, bytes)) => {
                    unsafe { write(base + rva, &bytes) };
                    log!("patches: {} bytes at {module}+{rva:#x}", bytes.len());
                }
                None => log!("patches: ignoring malformed '{module}!{patch}'"),
            }
            false
        });
        if pending.is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for (module, patch) in pending {
        log!("patches: {module} never loaded, '{patch}' not applied");
    }
}

/// File name of the process executable.
pub fn exe_name() -> String {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_string()
}
