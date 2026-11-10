//! Import hooks of the game's modules, kept for the DLLs of its directory it loads later.
//!
//! The window, display mode and OpenGL options hook the imports of the game's executable and
//! of the DLLs of its directory loaded when the payload starts ([`hook`]). A game loading its
//! renderer later (SiN, Quake II's engine: `ref_gl.dll` at the game's start, its window, mode
//! change and buffer swaps) gets them too: the game's `LoadLibrary` calls are hooked, and each
//! DLL of the game's directory they load gets the hooks recorded so far.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA, GetProcAddress};

/// The hooks applied: DLL, function, replacement, the original function's slot.
type Hook = (&'static str, &'static str, usize, &'static AtomicUsize);
static HOOKS: Mutex<Vec<Hook>> = Mutex::new(Vec::new());
/// The modules hooked (bases).
static DONE: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// The game's directory (lower case, with its last backslash).
static DIR: Mutex<String> = Mutex::new(String::new());

static ORIG_LOAD: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

fn path(base: usize) -> String {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(base as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n]).to_lowercase()
}

/// The real `dll!name` (its export).
fn export(dll: &str, name: &str) -> usize {
    let cdll = format!("{dll}\0");
    let cname = format!("{name}\0");
    let module = unsafe { GetModuleHandleA(cdll.as_ptr()) };
    (!module.is_null()).then(|| unsafe { GetProcAddress(module, cname.as_ptr()) }).flatten().map_or(0, |p| p as usize)
}

/// The module holding address `a`.
fn module_of(a: usize) -> usize {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    unsafe { GetModuleHandleExW(flags, a as *const u16, &mut module) };
    module as usize
}

fn apply(base: usize, (dll, name, f, orig): Hook) {
    if let Some(o) = unsafe { iat::hook_module(base, dll, name, f) } {
        if o == f {
            return;
        }
        if o != export(dll, name) && module_of(o) != crate::workarea::this_module() {
            // another DLL's hook (Unity Doorstop's GetProcAddress in the executable, BepInEx's
            // loader: winhttp.dll): kept, never the original of ours. That DLL calls the real
            // function through its own imports, hooked too: chained to its hook, endless.
            unsafe { iat::hook_module(base, dll, name, o) };
            return;
        }
        let prev = orig.load(Ordering::Relaxed);
        if prev == 0 {
            orig.store(o, Ordering::Relaxed);
        } else if o != prev && o != export(dll, name) {
            // the slot held another hook of the payload this one does not call (the Unity
            // runtime's GetProcAddress, Mono's DllImports resolved to the emulated serial ports
            // and dongles: replaced, the game opened the machine's real COM1): kept
            unsafe { iat::hook_module(base, dll, name, o) };
        }
    }
}

/// `dll!name` of the game's modules (and of the DLLs of its directory it loads later) replaced
/// by `f`, the original function in `orig` (from the DLL's exports when none imports it yet).
pub(crate) fn hook(dll: &'static str, name: &'static str, f: usize, orig: &'static AtomicUsize) {
    let modules = crate::workarea::game_modules(iat::game_module());
    {
        let mut dir = DIR.lock().unwrap();
        if dir.is_empty() {
            let exe = path(iat::game_module());
            *dir = exe.rsplit_once('\\').map(|(d, _)| format!("{d}\\")).unwrap_or_default();
        }
    }
    {
        let mut done = DONE.lock().unwrap();
        for &base in &modules {
            apply(base, (dll, name, f, orig));
            if !done.contains(&base) {
                done.push(base);
            }
        }
    }
    if orig.load(Ordering::Relaxed) == 0 {
        orig.store(export(dll, name), Ordering::Relaxed);
    }
    HOOKS.lock().unwrap().push((dll, name, f, orig));
    hook_loads(&modules);
}

/// The `LoadLibrary` imports of `modules` hooked (once each).
fn hook_loads(modules: &[usize]) {
    let hooks: [(&str, usize); 4] = [
        ("LoadLibraryA", load_a as *const () as usize),
        ("LoadLibraryW", load_w as *const () as usize),
        ("LoadLibraryExA", load_ex_a as *const () as usize),
        ("LoadLibraryExW", load_ex_w as *const () as usize),
    ];
    for &base in modules {
        for (i, (name, f)) in hooks.into_iter().enumerate() {
            if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", name, f) } {
                if o != f && ORIG_LOAD[i].load(Ordering::Relaxed) == 0 {
                    ORIG_LOAD[i].store(o, Ordering::Relaxed);
                }
            }
        }
    }
}

/// A module just loaded: the hooks, when it is a new DLL of the game's directory.
fn loaded(module: usize) {
    if module == 0 {
        return;
    }
    let mut done = DONE.lock().unwrap();
    if done.contains(&module) {
        return;
    }
    let dir = DIR.lock().unwrap().clone();
    let file = path(module);
    // not this payload, installed under a DLL's name the game loads (Unity's xinput1_3.dll): its
    // own calls are the real functions' (the fake desktop's real screen size)
    if dir.is_empty() || !file.starts_with(&dir) || module == crate::workarea::this_module() {
        return;
    }
    done.push(module);
    drop(done);
    let hooks = HOOKS.lock().unwrap().clone();
    for h in &hooks {
        apply(module, *h);
    }
    hook_loads(&[module]);
    log!("late hooks: {} ({} hooks)", file.rsplit('\\').next().unwrap_or(&file), hooks.len());
}

fn orig(i: usize) -> usize {
    let o = ORIG_LOAD[i].load(Ordering::Relaxed);
    if o != 0 {
        return o;
    }
    let names = [c"LoadLibraryA", c"LoadLibraryW", c"LoadLibraryExA", c"LoadLibraryExW"];
    let k32 = unsafe { GetModuleHandleA(c"kernel32.dll".as_ptr().cast()) };
    unsafe { GetProcAddress(k32, names[i].as_ptr().cast()) }.map_or(0, |p| p as usize)
}

unsafe extern "system" fn load_a(name: *const u8) -> usize {
    let f: unsafe extern "system" fn(*const u8) -> usize = unsafe { std::mem::transmute(orig(0)) };
    let m = unsafe { f(name) };
    loaded(m);
    m
}

unsafe extern "system" fn load_w(name: *const u16) -> usize {
    let f: unsafe extern "system" fn(*const u16) -> usize = unsafe { std::mem::transmute(orig(1)) };
    let m = unsafe { f(name) };
    loaded(m);
    m
}

unsafe extern "system" fn load_ex_a(name: *const u8, file: usize, flags: u32) -> usize {
    let f: unsafe extern "system" fn(*const u8, usize, u32) -> usize = unsafe { std::mem::transmute(orig(2)) };
    let m = unsafe { f(name, file, flags) };
    // not data or image resource loads (LOAD_LIBRARY_AS_DATAFILE..., the low bits set)
    if flags & 0x62 == 0 {
        loaded(m);
    }
    m
}

unsafe extern "system" fn load_ex_w(name: *const u16, file: usize, flags: u32) -> usize {
    let f: unsafe extern "system" fn(*const u16, usize, u32) -> usize = unsafe { std::mem::transmute(orig(3)) };
    let m = unsafe { f(name, file, flags) };
    if flags & 0x62 == 0 {
        loaded(m);
    }
    m
}
