//! Runtime patch finder: `WAL_MEMDIFF=<module>,...` compares the executable sections of these
//! loaded modules with their files on disk, 30 s and 90 s after start, and logs every changed
//! byte range (`memdiff:` lines: `module+rva file bytes -> memory bytes`). Shows what another
//! loader's DLL (a packed NxL2Core.dll wrapper...) patches in the game, to port it to profile
//! patches. Only code sections: x64 code is RIP-relative, so a relocated module still compares.

use std::collections::HashSet;

use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};

use crate::log;

const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;

pub fn start() {
    let Ok(modules) = std::env::var("WAL_MEMDIFF") else { return };
    let modules: Vec<String> = modules.split(',').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
    std::thread::spawn(move || {
        let mut seen = HashSet::new();
        for delay in [30, 60] {
            std::thread::sleep(std::time::Duration::from_secs(delay));
            for module in &modules {
                diff(module, &mut seen);
            }
        }
    });
}

fn diff(name: &str, seen: &mut HashSet<(String, usize)>) {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let base = unsafe { GetModuleHandleW(wide.as_ptr()) } as usize;
    if base == 0 {
        log!("memdiff: {name} not loaded");
        return;
    }
    let mut path = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(base as _, path.as_mut_ptr(), path.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&path[..n]);
    let Ok(file) = std::fs::read(&path) else {
        log!("memdiff: cannot read {path}");
        return;
    };
    let u16_at = |o: usize| file.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]])) as usize;
    let u32_at = |o: usize| file.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) as usize;
    let pe = u32_at(0x3C);
    let (count, opt) = (u16_at(pe + 6), u16_at(pe + 20));
    let mut ranges = 0;
    for i in 0..count {
        let s = pe + 24 + opt + 40 * i;
        let (vsize, rva, raw_size, raw) = (u32_at(s + 8), u32_at(s + 12), u32_at(s + 16), u32_at(s + 20));
        if u32_at(s + 36) as u32 & IMAGE_SCN_MEM_EXECUTE == 0 {
            continue;
        }
        let len = vsize.min(raw_size);
        let Some(disk) = file.get(raw..raw + len) else { continue };
        let mem = unsafe { std::slice::from_raw_parts((base + rva) as *const u8, len) };
        let mut i = 0;
        while i < len {
            if disk[i] == mem[i] {
                i += 1;
                continue;
            }
            let start = i;
            // a range ends after 8 equal bytes
            let mut end = i + 1;
            while end < len && (end - start) < 64 && disk[end..(end + 8).min(len)] != mem[end..(end + 8).min(len)] {
                end += 1;
            }
            let at = rva + start;
            if seen.insert((name.to_string(), at)) && ranges < 64 {
                let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02X}")).collect::<String>();
                log!("memdiff: {name}+{at:#x} {} -> {}", hex(&disk[start..end]), hex(&mem[start..end]));
                ranges += 1;
            }
            i = end;
        }
    }
    log!("memdiff: {name} compared ({ranges} new ranges)");
}
