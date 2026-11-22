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

/// `earlier`: the hooks recorded before this one.
fn apply(base: usize, (dll, name, f, orig): Hook, earlier: &[Hook]) {
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
        } else if o != prev && o != export(dll, name) && !earlier.iter().any(|h| h.0 == dll && h.1 == name && h.2 == o) {
            // the slot held another hook of the payload this one does not call (the Unity
            // runtime's GetProcAddress, Mono's DllImports resolved to the emulated serial ports
            // and dongles: replaced, the game opened the machine's real COM1): kept. One recorded
            // before it is in its chain, put on top of it in the game's modules, maybe through a
            // hook of those modules only (Crazy Speed Arcade on Batocera: the fake desktop's
            // CreateWindowExA calls the work area's, which calls the popup window's; kept in
            // Ogre's render system, its window was not fitted on the screen)
            unsafe { iat::hook_module(base, dll, name, o) };
        }
    }
}

/// `dll!name` of the game's modules (and of the DLLs of its directory it loads later) replaced
/// by `f`, the original function in `orig` (from the DLL's exports when none imports it yet).
pub fn hook(dll: &'static str, name: &'static str, f: usize, orig: &'static AtomicUsize) {
    let modules = crate::workarea::game_modules(iat::game_module());
    {
        let mut dir = DIR.lock().unwrap();
        if dir.is_empty() {
            let exe = path(iat::game_module());
            *dir = exe.rsplit_once('\\').map(|(d, _)| format!("{d}\\")).unwrap_or_default();
        }
    }
    // DONE not held while hooking: a hook may call the game's LoadLibrary, hooked by `loaded`
    let earlier = HOOKS.lock().unwrap().clone();
    for &base in &modules {
        apply(base, (dll, name, f, orig), &earlier);
        let mut done = DONE.lock().unwrap();
        if !done.contains(&base) {
            done.push(base);
        }
    }
    if orig.load(Ordering::Relaxed) == 0 {
        // the last hook recorded of the function, chained as in the modules it is applied to
        // later (Veggie Blast's IL-only executable imports nothing: the Sinden frame's
        // GetProcAddress replaced the Direct3D 9 shims' in its rasterizer, calling the real one)
        let below = earlier.iter().rev().find(|h| h.0 == dll && h.1 == name).map(|h| h.2);
        orig.store(below.unwrap_or_else(|| export(dll, name)), Ordering::Relaxed);
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
                if o != f && o != export("kernel32.dll", name) && module_of(o) == crate::workarea::this_module() {
                    // another hook of the payload (the Unity runtime's LoadLibrary, Mono's
                    // stand-ins for the emulated libraries of the other architecture: replaced,
                    // Disney Crossy Road's board never started, no coins): kept
                    unsafe { iat::hook_module(base, "kernel32.dll", name, o) };
                    continue;
                }
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
    for (i, h) in hooks.iter().enumerate() {
        apply(module, *h, &hooks[..i]);
    }
    hook_loads(&[module]);
    log!("late hooks: {} ({} hooks)", file.rsplit('\\').next().unwrap_or(&file), hooks.len());
}

/// Every DLL of the game's directory hooked as it loads, whoever loads it: a .NET host's
/// P/Invoke libraries are loaded by its runtime, never through a hooked `LoadLibrary` (Veggie
/// Blast's Virtools engine `ck2.dll`, then its Direct3D 9 rasterizer: no Sinden frame).
/// `LdrRegisterDllNotification`, called with the DLL's imports resolved, before its `DllMain`.
pub fn watch_all_loads() {
    #[repr(C)]
    struct Loaded {
        flags: u32,
        full_name: *const u16,
        base_name: *const u16,
        base: usize,
    }
    type Callback = unsafe extern "system" fn(u32, *const Loaded, *mut core::ffi::c_void);
    type Register = unsafe extern "system" fn(u32, Callback, *mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> i32;
    const LOADED: u32 = 1;

    unsafe extern "system" fn notified(reason: u32, data: *const Loaded, _context: *mut core::ffi::c_void) {
        if let Some(data) = (unsafe { data.as_ref() }).filter(|_| reason == LOADED) {
            loaded(data.base);
        }
    }

    let ntdll = unsafe { GetModuleHandleA(c"ntdll.dll".as_ptr().cast()) };
    let Some(register) = (unsafe { GetProcAddress(ntdll, c"LdrRegisterDllNotification".as_ptr().cast()) }) else {
        return;
    };
    let register: Register = unsafe { std::mem::transmute(register) };
    let mut cookie = std::ptr::null_mut();
    unsafe { register(0, notified, std::ptr::null_mut(), &mut cookie) };
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
