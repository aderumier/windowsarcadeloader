//! The cabinet's boot check libraries `kbt.dll` and `kld.dll` (not in the dumps): the game
//! finds them with `GetModuleHandle` and looks up their functions (`bt_get_ikey_status`, the
//! dongles' status; Bishi Bashi Channel also `k_bt0001` / `k_bt0002`), which this DLL exports:
//! `GetModuleHandle` answers this DLL for them. spice2x's stubs are the reference.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::iat;

/// This DLL, answered for the cabinet's boot check libraries.
static PAYLOAD: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_MODULE: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

/// The boot check libraries `kbt.dll` and `kld.dll` (not in the dump): this DLL, which exports
/// their functions the game looks up.
fn boot_check_library(name: &str) -> bool {
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    name.eq_ignore_ascii_case("kbt.dll") || name.eq_ignore_ascii_case("kld.dll")
}

unsafe extern "system" fn get_module_handle_a(name: *const u8) -> usize {
    if !name.is_null() && boot_check_library(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy()) {
        return PAYLOAD.load(Ordering::Relaxed);
    }
    let original: unsafe extern "system" fn(*const u8) -> usize = unsafe { std::mem::transmute(ORIG_GET_MODULE[0].load(Ordering::Relaxed)) };
    unsafe { original(name) }
}

unsafe extern "system" fn get_module_handle_w(name: *const u16) -> usize {
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        if boot_check_library(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) })) {
            return PAYLOAD.load(Ordering::Relaxed);
        }
    }
    let original: unsafe extern "system" fn(*const u16) -> usize = unsafe { std::mem::transmute(ORIG_GET_MODULE[1].load(Ordering::Relaxed)) };
    unsafe { original(name) }
}

/// kbt.dll's `k_bt0002(func, arg)`: runs the game's init `func(arg)` (after the cabinet's
/// checks, none here; spice2x's stub).
#[unsafe(no_mangle)]
pub extern "C" fn k_bt0002(func: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>, arg: *mut std::ffi::c_void) {
    if let Some(f) = func {
        unsafe { f(arg) };
    }
}

/// kbt.dll's dongle (iKey) status: two keys present.
#[unsafe(no_mangle)]
pub extern "C" fn k_bt0001(status: *mut u32) -> i32 {
    bt_get_ikey_status(status)
}

#[unsafe(no_mangle)]
pub extern "C" fn bt_get_ikey_status(status: *mut u32) -> i32 {
    if status.is_null() {
        return 0;
    }
    // two 16-byte entries (spice2x's ikey_status: u32, four bytes, u32, u32): bytes 4 and 6
    // set, the last u32 all ones
    unsafe { std::ptr::write_bytes(status as *mut u8, 0, 32) };
    for i in 0..2 {
        let entry = unsafe { (status as *mut u8).add(i * 16) };
        unsafe {
            *entry.add(4) = 1;
            *entry.add(6) = 1;
            std::ptr::write_unaligned(entry.add(12) as *mut u32, !0);
        }
    }
    0
}

/// Points the game's `GetModuleHandleA` / `GetModuleHandleW` imports at the answers above.
pub(crate) fn init(module: crate::HINSTANCE) {
    PAYLOAD.store(module as usize, Ordering::Relaxed);
    for (name, f) in [("GetModuleHandleA", get_module_handle_a as *const () as usize), ("GetModuleHandleW", get_module_handle_w as *const () as usize)] {
        if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            ORIG_GET_MODULE[(name == "GetModuleHandleW") as usize].store(o, Ordering::Relaxed);
        }
    }
}
