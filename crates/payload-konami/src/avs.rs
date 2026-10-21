//! `WAL_KONAMI_AVS_LOG=1`: the AVS log (the game system's boot steps, errors, warnings) in the
//! payload log. The game boots AVS with a log writer that drops every line (release build):
//! its `avs_boot` import is hooked to pass ours instead.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

static ORIG_AVS_BOOT: AtomicUsize = AtomicUsize::new(0);

type LogWriter = unsafe extern "C" fn(*mut c_void, *const u8, u32) -> u32;
type AvsBoot = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void, u32, LogWriter, *mut c_void);

unsafe extern "C" fn log_writer(_ctx: *mut c_void, chars: *const u8, n: u32) -> u32 {
    if !chars.is_null() && n != 0 {
        let bytes = unsafe { std::slice::from_raw_parts(chars, n as usize) };
        for line in String::from_utf8_lossy(bytes).lines().filter(|l| !l.trim().is_empty()) {
            log!("avs: {line}");
        }
    }
    n
}

unsafe extern "C" fn avs_boot(
    config: *mut c_void, heap: *mut c_void, heap_size: u32, user_heap: *mut c_void, user_size: u32,
    _writer: LogWriter, ctx: *mut c_void,
) {
    let original: AvsBoot = unsafe { std::mem::transmute(ORIG_AVS_BOOT.load(Ordering::Relaxed)) };
    unsafe { original(config, heap, heap_size, user_heap, user_size, log_writer, ctx) }
}

/// byy.exe's boot config sets `/config/log/level` to 0 (no log): `push 0` at 0x401743, before
/// `push "/config/log/level"`. Made 4 (misc: every line).
unsafe fn log_level() {
    let at = (unsafe { GetModuleHandleW(std::ptr::null()) } as usize + 0x1743) as *mut u8;
    let expected: [u8; 7] = [0x6a, 0x00, 0x68, 0x08, 0x32, 0x40, 0x00];
    if unsafe { std::slice::from_raw_parts(at, 7) } != expected {
        log!("avs: log level signature mismatch, the game's level kept");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at.add(1).cast(), 1, PAGE_EXECUTE_READWRITE, &mut old);
        *at.add(1) = 4;
        VirtualProtect(at.add(1).cast(), 1, old, &mut old);
    }
}

pub fn init() {
    if !std::env::var("WAL_KONAMI_AVS_LOG").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    unsafe { log_level() };
    match unsafe { iat::hook("libavs-win32.dll", "avs_boot", avs_boot as *const () as usize) } {
        Some(o) => ORIG_AVS_BOOT.store(o, Ordering::Relaxed),
        None => log!("avs: avs_boot import not found, no AVS log"),
    }
}
