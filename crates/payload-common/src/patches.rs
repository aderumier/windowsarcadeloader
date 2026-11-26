//! Game code patches from the profile: `WAL_PATCHES=<rva>:<hex bytes>,...` written at
//! `image base + rva` of the game executable, e.g. `0xFF0A0:B801000000C3` (return true).
//! Keeps game-specific fixes out of the payload code.
//!
//! `WAL_PATCHES_EXE=<file name>,...` limits them to those executables (the same program under
//! two names: Harikiri Professional Baseball's launcher.exe, started again as game.exe): the
//! payload is also loaded by the other programs of the game directory (Tottemo E Mahjong's
//! TestMode.exe was corrupted by the patches of game2.exe).
//!
//! `<module>!<rva>:<hex bytes>` patches a DLL the game loads later (an Unreal plugin's
//! library): applied by a thread as soon as the module is loaded (checked every 50 ms, 10 min).

use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

use crate::log;

pub fn apply() {
    let Ok(spec) = std::env::var("WAL_PATCHES") else { return };
    if let Ok(target) = std::env::var("WAL_PATCHES_EXE") {
        let exe = exe_name();
        if !target.split(',').any(|t| exe.eq_ignore_ascii_case(t.trim())) {
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

/// A jump from the game's code at `at` to `hook`, once its first bytes are `prologue` (whole,
/// position independent instructions, 5 bytes at least): the trampoline (those bytes, then a jump
/// back after them), the original function to call, or `None` (another build: left as it is).
///
/// # Safety
/// `hook` must have the replaced function's signature and calling convention.
pub unsafe fn detour(at: usize, prologue: &[u8], hook: usize) -> Option<usize> {
    if prologue.len() < 5 || !crate::crash::readable(at, prologue.len()) || unsafe { std::slice::from_raw_parts(at as *const u8, prologue.len()) } != prologue {
        return None;
    }
    // the jump to the hook: rel32 when the hook is near, else (64-bit) an absolute jump, when
    // the prologue has room for it (14 bytes)
    let rel = (hook as i64).wrapping_sub(at as i64 + 5);
    // (32-bit: rel32 reaches the whole address space, modulo 2^32)
    let near = cfg!(target_pointer_width = "32") || i32::try_from(rel).is_ok();
    if !near && prologue.len() < 14 {
        log!("patches: hook {hook:#x} too far from {at:#x} for a {} byte prologue", prologue.len());
        return None;
    }
    unsafe {
        // the trampoline: the prologue, then an absolute jump back after it (the allocation may be
        // more than 2 GB away from the code in a 64-bit process)
        let t = VirtualAlloc(std::ptr::null(), prologue.len() + 14, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
        if t.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(prologue.as_ptr(), t, prologue.len());
        let back = t.add(prologue.len());
        let resume = at + prologue.len();
        let back_rel = (resume as i64).wrapping_sub(back as i64 + 5);
        if cfg!(target_pointer_width = "32") || i32::try_from(back_rel).is_ok() {
            *back = 0xE9;
            std::ptr::write_unaligned(back.add(1) as *mut i32, back_rel as i32);
        } else {
            // jmp [rip+0]; dq resume
            std::ptr::copy_nonoverlapping([0xFFu8, 0x25, 0, 0, 0, 0].as_ptr(), back, 6);
            std::ptr::write_unaligned(back.add(6) as *mut u64, resume as u64);
        }
        let mut jump = vec![0x90u8; prologue.len()];
        if near {
            jump[0] = 0xE9;
            jump[1..5].copy_from_slice(&(rel as i32).to_le_bytes());
        } else {
            jump[..6].copy_from_slice(&[0xFF, 0x25, 0, 0, 0, 0]);
            jump[6..14].copy_from_slice(&(hook as u64).to_le_bytes());
        }
        write(at, &jump);
        Some(t as usize)
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
