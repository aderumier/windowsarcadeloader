//! Initial D THE ARCADE's cabinet checks, in the game and in its IndRun plugin (`IndRun.dll`,
//! loaded later): segatools' `idachook/indrun.c` (public domain). The game sees the cabinet's
//! Windows: no process list (the Wine processes beside it), the computer name
//! `ACAE01A99999999`, the user `AppUser`, Windows 10 1809, no mouse buttons, the current
//! directory `X`.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);
static GET_VERSION_EX: AtomicUsize = AtomicUsize::new(0);
static VERIFY_VERSION_INFO: AtomicUsize = AtomicUsize::new(0);
static LOAD_LIBRARY_W: AtomicUsize = AtomicUsize::new(0);
static LOAD_LIBRARY_EX_W: AtomicUsize = AtomicUsize::new(0);

const SM_CMOUSEBUTTONS: i32 = 43;

unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    if index == SM_CMOUSEBUTTONS {
        return 0;
    }
    let f: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
    unsafe { f(index) }
}

/// Copies `s` with its terminator into `buf`, `len` set to its length.
unsafe fn put_wide(buf: *mut u16, len: *mut u32, s: &str) -> i32 {
    let wide: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
    if buf.is_null() || len.is_null() {
        return 0;
    }
    if unsafe { *len } < wide.len() as u32 {
        unsafe { *len = wide.len() as u32 };
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), buf, wide.len());
        *len = wide.len() as u32 - 1;
    }
    1
}

unsafe extern "system" fn get_computer_name_w(buf: *mut u16, len: *mut u32) -> i32 {
    unsafe { put_wide(buf, len, "ACAE01A99999999") }
}

unsafe extern "system" fn get_user_name_w(buf: *mut u16, len: *mut u32) -> i32 {
    // the user name's length counts its terminator
    let r = unsafe { put_wide(buf, len, "AppUser") };
    if r != 0 {
        unsafe { *len += 1 };
    }
    r
}

unsafe extern "system" fn get_current_directory_w(size: u32, buf: *mut u16) -> u32 {
    if size >= 2 && !buf.is_null() {
        unsafe {
            *buf = b'X' as u16;
            *buf.add(1) = 0;
        }
    }
    1
}

/// `OSVERSIONINFOW`: size, major, minor, build...
unsafe extern "system" fn get_version_ex_w(info: *mut u32) -> i32 {
    let f: unsafe extern "system" fn(*mut u32) -> i32 = unsafe { std::mem::transmute(GET_VERSION_EX.load(Ordering::Relaxed)) };
    let r = unsafe { f(info) };
    if r != 0 {
        unsafe {
            *info.add(1) = 10;
            *info.add(2) = 0;
            *info.add(3) = 17763;
        }
    }
    r
}

unsafe extern "system" fn verify_version_info_w(info: *mut u32, mask: u32, condition: u64) -> i32 {
    let f: unsafe extern "system" fn(*mut u32, u32, u64) -> i32 = unsafe { std::mem::transmute(VERIFY_VERSION_INFO.load(Ordering::Relaxed)) };
    if !info.is_null() && unsafe { *info.add(3) } == 17763 {
        return 1;
    }
    unsafe { f(info, mask, condition) }
}

unsafe extern "system" fn enum_processes(_ids: *mut u32, _size: u32, needed: *mut u32) -> i32 {
    if !needed.is_null() {
        unsafe { *needed = 0 };
    }
    1
}

/// The hooks in one module's imports.
fn hook(base: usize) {
    unsafe {
        let set = |slot: &AtomicUsize, orig: Option<usize>| {
            if let Some(o) = orig
                && slot.load(Ordering::Relaxed) == 0
            {
                slot.store(o, Ordering::Relaxed);
            }
        };
        set(&GET_SYSTEM_METRICS, iat::hook_module(base, "user32.dll", "GetSystemMetrics", get_system_metrics as *const () as usize));
        iat::hook_module(base, "kernel32.dll", "GetComputerNameW", get_computer_name_w as *const () as usize);
        iat::hook_module(base, "kernel32.dll", "GetCurrentDirectoryW", get_current_directory_w as *const () as usize);
        set(&GET_VERSION_EX, iat::hook_module(base, "kernel32.dll", "GetVersionExW", get_version_ex_w as *const () as usize));
        set(&VERIFY_VERSION_INFO, iat::hook_module(base, "kernel32.dll", "VerifyVersionInfoW", verify_version_info_w as *const () as usize));
        iat::hook_module(base, "kernel32.dll", "K32EnumProcesses", enum_processes as *const () as usize);
        iat::hook_module(base, "advapi32.dll", "GetUserNameW", get_user_name_w as *const () as usize);
    }
}

/// A module just loaded: IndRun.dll gets the hooks.
fn loaded(module: usize, name: *const u16) {
    if module == 0 || name.is_null() {
        return;
    }
    let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
    let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }).to_ascii_lowercase();
    if name.ends_with("indrun.dll") {
        log!("indrun: IndRun.dll hooked");
        hook(module);
        #[cfg(target_arch = "x86_64")]
        crate::indstate::init(module);
    }
}

unsafe extern "system" fn load_library_w(name: *const u16) -> usize {
    let f: unsafe extern "system" fn(*const u16) -> usize = unsafe { std::mem::transmute(LOAD_LIBRARY_W.load(Ordering::Relaxed)) };
    let m = unsafe { f(name) };
    loaded(m, name);
    m
}

unsafe extern "system" fn load_library_ex_w(name: *const u16, file: usize, flags: u32) -> usize {
    let f: unsafe extern "system" fn(*const u16, usize, u32) -> usize = unsafe { std::mem::transmute(LOAD_LIBRARY_EX_W.load(Ordering::Relaxed)) };
    let m = unsafe { f(name, file, flags) };
    loaded(m, name);
    m
}

pub(crate) fn init() {
    let exe = iat::game_module();
    hook(exe);
    unsafe {
        if let Some(o) = iat::hook_module(exe, "kernel32.dll", "LoadLibraryW", load_library_w as *const () as usize) {
            LOAD_LIBRARY_W.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook_module(exe, "kernel32.dll", "LoadLibraryExW", load_library_ex_w as *const () as usize) {
            LOAD_LIBRARY_EX_W.store(o, Ordering::Relaxed);
        }
    }
}
