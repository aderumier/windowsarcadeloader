//! The ABAAS link client in its own DLL (`abaaslink.dll`: Kasiori, Goonya Fighter, Koihime
//! Enbu), the cabinets' in-store versus matching. Without a link server (none is emulated:
//! `WAL_APM3_ABAAS_LINK` unset) the client reports it unavailable and the games show it ("in-store
//! matching unavailable", the network icon not green). `WAL_APM3_LINK_READY` (default on): once the
//! DLL is loaded, its `linkIsAvailable` / `linkIsChecked` answer 1 (the games then play against the
//! CPU as before, without the warning).

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// `mov eax, 1; ret` (x86: stdcall, `ret 4`: one argument, the context).
#[cfg(target_pointer_width = "64")]
const TRUE: &[u8] = &[0xb8, 1, 0, 0, 0, 0xc3];
#[cfg(target_pointer_width = "32")]
const TRUE: &[u8] = &[0xb8, 1, 0, 0, 0, 0xc2, 4, 0];

pub(crate) fn start() {
    if crate::env("WAL_APM3_LINK_READY").is_some_and(|v| v == "0") || crate::env("WAL_APM3_ABAAS_LINK").is_some() {
        return;
    }
    std::thread::spawn(|| {
        for _ in 0..150_000 {
            let module = unsafe { GetModuleHandleA(c"abaaslink.dll".as_ptr().cast()) };
            if !module.is_null() {
                for name in ["linkIsAvailable", "linkIsChecked"] {
                    let plain = format!("{name}\0");
                    let decorated = format!("_{name}@4\0");
                    let f = unsafe { GetProcAddress(module, plain.as_ptr()).or_else(|| GetProcAddress(module, decorated.as_ptr())) };
                    let Some(f) = f else {
                        log!("link: abaaslink.dll has no {name}");
                        continue;
                    };
                    let at = f as usize as *mut u8;
                    let mut old = 0;
                    unsafe {
                        VirtualProtect(at.cast(), TRUE.len(), PAGE_EXECUTE_READWRITE, &mut old);
                        std::ptr::copy_nonoverlapping(TRUE.as_ptr(), at, TRUE.len());
                        VirtualProtect(at.cast(), TRUE.len(), old, &mut old);
                    }
                }
                log!("link: abaaslink.dll loaded: the link reported available (no link server)");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });
}
