//! The Mono `DllImport`s of a Unity game, pointed at the payload's functions.
//!
//! Mono resolves a `DllImport` with `LoadLibrary` and `GetProcAddress`: its `GetProcAddress`
//! import is hooked in Mono's runtime (`mono-2.0-bdwgc.dll` of Unity 2017 and later, `mono.dll`
//! of the older ones) as soon as Unity has loaded it, and the game's resolver gives the payload's
//! function for an export name (the cabinet's dongle and I/O DLLs, `kernel32`'s serial functions
//! for a port the payload emulates). Mono resolves a `DllImport` at its first call: the game's
//! hardware code runs long after the runtime is loaded.
//!
//! An IL2CPP game's `DllImport`s are resolved the same way, by `GameAssembly.dll` (its own
//! `LoadLibrary` and `GetProcAddress` imports), or by Unity 2021's `baselib.dll`
//! (`Baselib_DynamicLibrary_GetFunction`, which `GameAssembly.dll` calls): hooked the same.

use std::ffi::{CStr, c_char, c_void};
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress};

/// The payload's function for an export (`name`) of a module (`module`: its file name, lower
/// case: `kernel32.dll`, `gameport.dll`), if any.
pub type Resolver = fn(name: &str, module: &str) -> Option<usize>;

/// The file name of the module at `module`, lower case.
fn module_name(module: *mut c_void) -> String {
    let mut path = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module as _, path.as_mut_ptr(), path.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&path[..n]).to_lowercase();
    path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
}

static RESOLVER: OnceLock<Resolver> = OnceLock::new();
/// Mono's `GetProcAddress`.
static ORIG: AtomicUsize = AtomicUsize::new(0);

/// The modules the payload emulates whole (file names, lower case), and the stand-in handles
/// given for them (see `stand_in`).
static STAND_INS: OnceLock<&'static [&'static str]> = OnceLock::new();
static STAND_IN_HANDLES: Mutex<Vec<(usize, &'static str)>> = Mutex::new(Vec::new());
/// Mono's `LoadLibraryW` and `LoadLibraryA`.
static ORIG_LOAD_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_A: AtomicUsize = AtomicUsize::new(0);

/// Modules the payload emulates whole: when Mono cannot load one (missing, or of the other
/// architecture: Disney Crossy Road, a 64-bit game whose dump has the 32-bit board and dongle
/// libraries under the names it imports), a stand-in handle is given, its functions the
/// resolver's.
pub fn stand_in(modules: &'static [&'static str]) {
    let _ = STAND_INS.set(modules);
}

/// The stand-in module's name of a handle, if it is one.
fn stand_in_name(module: *mut c_void) -> Option<&'static str> {
    STAND_IN_HANDLES.lock().unwrap().iter().find(|(h, _)| *h == module as usize).map(|(_, n)| *n)
}

/// A load failed: the stand-in handle of an emulated module (file name of `path`, `.dll` added).
fn stand_in_for(path: &str) -> usize {
    let Some(modules) = STAND_INS.get() else { return 0 };
    let mut file = path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
    if !file.contains('.') {
        file.push_str(".dll");
    }
    let Some(name) = modules.iter().find(|m| **m == file) else { return 0 };
    let mut handles = STAND_IN_HANDLES.lock().unwrap();
    if let Some((h, _)) = handles.iter().find(|(_, n)| n == name) {
        return *h;
    }
    // a page of its own: an address no module has (no PE header: the real GetProcAddress fails)
    let h = unsafe {
        windows_sys::Win32::System::Memory::VirtualAlloc(
            std::ptr::null(),
            4096,
            windows_sys::Win32::System::Memory::MEM_COMMIT | windows_sys::Win32::System::Memory::MEM_RESERVE,
            windows_sys::Win32::System::Memory::PAGE_READONLY,
        )
    } as usize;
    if h != 0 {
        log!("mono: {path} not loadable: emulated by the payload");
        handles.push((h, name));
    }
    h
}

unsafe extern "system" fn load_library_w(path: *const u16) -> usize {
    let orig: unsafe extern "system" fn(*const u16) -> usize = unsafe { std::mem::transmute(ORIG_LOAD_W.load(Ordering::Relaxed)) };
    let h = unsafe { orig(path) };
    if h != 0 || path.is_null() {
        return h;
    }
    let len = (0..).take_while(|&i| unsafe { *path.add(i) } != 0).count();
    stand_in_for(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(path, len) }))
}

unsafe extern "system" fn load_library_a(path: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_LOAD_A.load(Ordering::Relaxed)) };
    let h = unsafe { orig(path) };
    if h != 0 || path.is_null() {
        return h;
    }
    stand_in_for(&unsafe { CStr::from_ptr(path) }.to_string_lossy())
}

unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_char) -> usize = unsafe { std::mem::transmute(ORIG.load(Ordering::Relaxed)) };
    // an ordinal
    if (name as usize) >> 16 == 0 {
        return unsafe { orig(module, name) };
    }
    let wanted = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
    let stand_in = stand_in_name(module);
    let module_name = stand_in.map_or_else(|| module_name(module), str::to_string);
    match RESOLVER.get().and_then(|resolve| resolve(wanted, &module_name)) {
        Some(f) => {
            log!("mono: {wanted} resolved to the payload's");
            f
        }
        None if stand_in.is_some() => {
            log!("mono: {module_name}!{wanted} not emulated");
            0
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
    // now if Unity has loaded the runtime (the payload loaded by Mono itself), else at its load
    if !hook_runtime() {
        std::thread::spawn(|| {
            while !hook_runtime() {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
    }
}

/// Hooks the runtime's `GetProcAddress`, if it is loaded.
fn hook_runtime() -> bool {
    for dll in [windows_sys::w!("mono-2.0-bdwgc.dll"), windows_sys::w!("mono.dll"), windows_sys::w!("GameAssembly.dll")] {
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
        if STAND_INS.get().is_some() {
            if let Some(o) = unsafe { iat::hook_module(mono, "kernel32.dll", "LoadLibraryW", load_library_w as *const () as usize) } {
                ORIG_LOAD_W.store(o, Ordering::Relaxed);
            }
            if let Some(o) = unsafe { iat::hook_module(mono, "kernel32.dll", "LoadLibraryA", load_library_a as *const () as usize) } {
                ORIG_LOAD_A.store(o, Ordering::Relaxed);
            }
        }
        // Unity 2021's IL2CPP: the DllImports' functions from baselib.dll's GetProcAddress
        let baselib = unsafe { GetModuleHandleW(windows_sys::w!("baselib.dll")) } as usize;
        if baselib != 0 && unsafe { iat::hook_module(baselib, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) }.is_some() {
            log!("mono: baselib.dll's GetProcAddress hooked");
        }
        return true;
    }
    false
}
