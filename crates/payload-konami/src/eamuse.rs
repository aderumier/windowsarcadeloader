//! e-Amusement offline. The boot's e-AMUSEMENT CONNECTION CHECK (bootmode.dll, its step 13)
//! waits for the network's board state to leave "connecting" when the test menu's e-Amusement
//! option is on (the factory setting): without Konami's servers it never does and the game stays
//! on "application starting". Its e-Amusement-off branch is taken instead (`jne` at rva 0x231b on
//! the setting, NOPed): the game runs in local mode (its attract shows it), as a cabinet set
//! offline. bootmode.dll is loaded by the game after the payload: patched once it appears.

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

pub fn init() {
    std::thread::spawn(|| {
        let name: Vec<u16> = "bootmode.dll".encode_utf16().chain(std::iter::once(0)).collect();
        // the boot screen comes up within a few seconds; give up after a minute
        for _ in 0..12000 {
            let base = unsafe { GetModuleHandleW(name.as_ptr()) } as usize;
            if base != 0 {
                let at = (base + 0x231b) as *mut u8;
                if unsafe { std::slice::from_raw_parts(at, 2) } != [0x75, 0x10] {
                    log!("eamuse: bootmode signature mismatch, e-Amusement check kept");
                    return;
                }
                let mut old = 0;
                unsafe {
                    VirtualProtect(at.cast(), 2, PAGE_EXECUTE_READWRITE, &mut old);
                    *at = 0x90;
                    *at.add(1) = 0x90;
                    VirtualProtect(at.cast(), 2, old, &mut old);
                }
                log!("eamuse: offline (boot e-Amusement check skipped)");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
}
