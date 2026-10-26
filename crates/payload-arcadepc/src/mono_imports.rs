//! The Mono `DllImport`s of a Unity game, pointed at the payload's functions.
//!
//! Mono resolves a `DllImport` with `LoadLibrary` and `GetProcAddress`: its `GetProcAddress`
//! import is hooked in Mono's runtime (`mono-2.0-bdwgc.dll` of Unity 2017 and later, `mono.dll`
//! of the older ones) as soon as Unity has loaded it, and the game's resolver gives the payload's
//! function for an export name (the cabinet's dongle and I/O DLLs, `kernel32`'s serial functions
//! for a port the payload emulates). Mono resolves a `DllImport` at its first call: the game's
//! hardware code runs long after the runtime is loaded.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

/// The payload's function for an export (`name`) of a module (`kernel32`: whether the module is
/// kernel32.dll), if any.
pub type Resolver = fn(name: &str, kernel32: bool) -> Option<usize>;

static RESOLVER: OnceLock<Resolver> = OnceLock::new();
/// Mono's `GetProcAddress`.
static ORIG: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_char) -> usize = unsafe { std::mem::transmute(ORIG.load(Ordering::Relaxed)) };
    // an ordinal
    if (name as usize) >> 16 == 0 {
        return unsafe { orig(module, name) };
    }
    let wanted = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
    let kernel32 = module == unsafe { GetModuleHandleW(windows_sys::w!("kernel32.dll")) };
    match RESOLVER.get().and_then(|resolve| resolve(wanted, kernel32)) {
        Some(f) => {
            log!("mono: {wanted} resolved to the payload's");
            f
        }
        None => unsafe { orig(module, name) },
    }
}

/// Points the game's `DllImport`s at `resolve`'s functions.
pub fn hook(resolve: Resolver) {
    if RESOLVER.set(resolve).is_err() {
        return;
    }
    let kernel32 = unsafe { GetModuleHandleW(windows_sys::w!("kernel32.dll")) };
    if let Some(f) = unsafe { GetProcAddress(kernel32, c"GetProcAddress".as_ptr().cast()) } {
        ORIG.store(f as usize, Ordering::Relaxed);
    }
    std::thread::spawn(|| {
        loop {
            for dll in [windows_sys::w!("mono-2.0-bdwgc.dll"), windows_sys::w!("mono.dll")] {
                let mono = unsafe { GetModuleHandleW(dll) } as usize;
                if mono == 0 {
                    continue;
                }
                match unsafe { iat::hook_module(mono, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
                    Some(o) => {
                        ORIG.store(o, Ordering::Relaxed);
                        log!("mono: GetProcAddress hooked");
                    }
                    None => log!("mono: the runtime does not import GetProcAddress: no hardware emulated"),
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
}
