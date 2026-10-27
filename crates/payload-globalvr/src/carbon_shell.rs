//! Need for Speed Carbon's cabinet frontend (the Anark player `AMPlayer.exe` running
//! `shell.am`): the cabinet's monitor programs it talks to, started with it.
//!
//! On the cabinet `GVRBoot.exe` starts the Hercules monitors with the frontend. Its
//! `HerculesPlugIn.dll` needs the coin monitor (`GVRCoinMonitor.exe`: coins and credits, shared
//! memory `GVRCoinMonitor_map`) and the volume monitor (`GVRVolumeMonitor.exe`: the attract reads
//! the volume every few frames, each read waited 1 s for an answer without it: the attract drew
//! a frame a second). Both read the USBIO board through `USBIOExtreme.dll`: the payload,
//! installed next to them, feeds them the virtual sticks' coins. Loaded in the frontend, the
//! payload starts them (`WAL_GLOBALVR_CARBON_MONITORS`: the monitors' folder, relative to the
//! frontend's executable).
//!
//! The startup checks (`Shell.am`'s script, "Please Wait") wait 45 s before reading the I/O
//! board (`ioBoardDelay = presentation.realTime + 45.0`: the cabinet's USB board coming up), the
//! payload's board answers at once: the player's script compilations (its SpiderMonkey's
//! `JS_CompileUC*` / `JS_EvaluateUC*`, called directly) get a copy of the script with 1 s. The
//! decompressed `.am` text itself stays untouched: the player checks it (an edited script did
//! not run).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};

const MONITORS: &[&str] = &["GVRCoinMonitor.exe", "GVRVolumeMonitor.exe"];

/// The player's script compilations (SpiderMonkey, UTF-16 source), as image offsets: each
/// function and its calls.
const COMPILE_SCRIPT: usize = 0x26c080;
const COMPILE_SCRIPT_CALLS: [usize; 7] = [0x26c5b6, 0x26de9b, 0x26dee0, 0x26ea58, 0x2733bf, 0x27c336, 0x2ac69f];
const COMPILE_FUNCTION: usize = 0x26c250;
const COMPILE_FUNCTION_CALLS: [usize; 3] = [0x26df9a, 0x26dfef, 0x26ebc7];
const EVALUATE_SCRIPT: usize = 0x26c590;
const EVALUATE_SCRIPT_CALLS: [usize; 3] = [0x26e180, 0x26e1c5, 0x26ec1d];
/// The I/O board delay of the startup checks, and its replacement (same length).
const BOARD_DELAY: &str = "presentation.realTime + 45.0;";
const BOARD_DELAY_NEW: &str = "presentation.realTime +  1.0;";

static ORIG_COMPILE_SCRIPT: AtomicUsize = AtomicUsize::new(0);
static ORIG_COMPILE_FUNCTION: AtomicUsize = AtomicUsize::new(0);
static ORIG_EVALUATE_SCRIPT: AtomicUsize = AtomicUsize::new(0);

type P = *mut c_void;
/// `JS_CompileUCScriptForPrincipals(cx, obj, principals, chars, length, filename, lineno)`.
type CompileScript = unsafe extern "C" fn(P, P, P, *const u16, usize, P, u32) -> P;
/// `JS_CompileUCFunctionForPrincipals(cx, obj, principals, name, nargs, argnames, chars,
/// length, filename, lineno)`.
type CompileFunction = unsafe extern "C" fn(P, P, P, P, u32, P, *const u16, usize, P, u32) -> P;
/// `JS_EvaluateUCScriptForPrincipals(cx, obj, principals, chars, length, filename, lineno,
/// rval)`.
type EvaluateScript = unsafe extern "C" fn(P, P, P, *const u16, usize, P, u32, P) -> i32;

/// The script with the board delay replaced (a copy kept for good: the engine may refer to
/// its source), else the script itself.
fn script(chars: *const u16, length: usize) -> *const u16 {
    if chars.is_null() || length < BOARD_DELAY.len() {
        return chars;
    }
    let text = unsafe { std::slice::from_raw_parts(chars, length) };
    let pattern: Vec<u16> = BOARD_DELAY.encode_utf16().collect();
    let Some(i) = text.windows(pattern.len()).position(|w| w == pattern) else { return chars };
    let mut copy = text.to_vec();
    for (dst, src) in copy[i..].iter_mut().zip(BOARD_DELAY_NEW.encode_utf16()) {
        *dst = src;
    }
    log!("carbon shell: startup I/O board delay 45 s -> 1 s");
    Box::leak(copy.into_boxed_slice()).as_ptr()
}

unsafe extern "C" fn compile_script(cx: P, obj: P, pr: P, chars: *const u16, length: usize, file: P, line: u32) -> P {
    let f: CompileScript = unsafe { std::mem::transmute(ORIG_COMPILE_SCRIPT.load(Ordering::Relaxed)) };
    unsafe { f(cx, obj, pr, script(chars, length), length, file, line) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn compile_function(
    cx: P,
    obj: P,
    pr: P,
    name: P,
    nargs: u32,
    argnames: P,
    chars: *const u16,
    length: usize,
    file: P,
    line: u32,
) -> P {
    let f: CompileFunction = unsafe { std::mem::transmute(ORIG_COMPILE_FUNCTION.load(Ordering::Relaxed)) };
    unsafe { f(cx, obj, pr, name, nargs, argnames, script(chars, length), length, file, line) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn evaluate_script(cx: P, obj: P, pr: P, chars: *const u16, length: usize, file: P, line: u32, rval: P) -> i32 {
    let f: EvaluateScript = unsafe { std::mem::transmute(ORIG_EVALUATE_SCRIPT.load(Ordering::Relaxed)) };
    unsafe { f(cx, obj, pr, script(chars, length), length, file, line, rval) }
}

/// Whether all `calls` call `function` (image offsets).
fn calls_found(base: usize, function: usize, calls: &[usize]) -> bool {
    calls.iter().all(|&site| {
        let code = unsafe { std::slice::from_raw_parts((base + site) as *const u8, 5) };
        let rel = i32::from_le_bytes(code[1..5].try_into().unwrap());
        code[0] == 0xE8 && (base + site + 5).wrapping_add(rel as usize) == base + function
    })
}

/// Repoints `calls` (image offsets) to `hook`.
fn repoint(base: usize, calls: &[usize], hook: usize) {
    for &site in calls {
        let at = base + site;
        let rel = hook.wrapping_sub(at + 5) as i32;
        let mut old = 0;
        unsafe {
            VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::write_unaligned((at + 1) as *mut i32, rel);
            VirtualProtect(at as *const c_void, 5, old, &mut old);
        }
    }
}

/// Repoints the player's script compilations to [`compile_script`], [`compile_function`] and
/// [`evaluate_script`], if their calls are where expected.
fn hook_scripts() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if crate::exe_size() < EVALUATE_SCRIPT + 5
        || !calls_found(base, COMPILE_SCRIPT, &COMPILE_SCRIPT_CALLS)
        || !calls_found(base, COMPILE_FUNCTION, &COMPILE_FUNCTION_CALLS)
        || !calls_found(base, EVALUATE_SCRIPT, &EVALUATE_SCRIPT_CALLS)
    {
        log!("carbon shell: script compilation calls not found, startup delay kept");
        return;
    }
    ORIG_COMPILE_SCRIPT.store(base + COMPILE_SCRIPT, Ordering::Relaxed);
    ORIG_COMPILE_FUNCTION.store(base + COMPILE_FUNCTION, Ordering::Relaxed);
    ORIG_EVALUATE_SCRIPT.store(base + EVALUATE_SCRIPT, Ordering::Relaxed);
    repoint(base, &COMPILE_SCRIPT_CALLS, compile_script as *const () as usize);
    repoint(base, &COMPILE_FUNCTION_CALLS, compile_function as *const () as usize);
    repoint(base, &EVALUATE_SCRIPT_CALLS, evaluate_script as *const () as usize);
}

pub(crate) fn init() {
    let Ok(dir) = std::env::var("WAL_GLOBALVR_CARBON_MONITORS") else { return };
    if crate::frontend_fit::exe_name() != "amplayer.exe" {
        return;
    }
    hook_scripts();
    let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else {
        return;
    };
    let dir = exe_dir.join(dir);
    // not from DllMain (the loader lock): a thread, run once the frontend's plug-in is loaded
    std::thread::spawn(move || start(&dir));
}

fn start(dir: &std::path::Path) {
    for name in MONITORS {
        let path = dir.join(name);
        let mut cmd: Vec<u16> = format!("\"{}\"", path.display()).encode_utf16().chain(Some(0)).collect();
        let cwd: Vec<u16> = dir.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..unsafe { std::mem::zeroed() } };
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NO_WINDOW,
                std::ptr::null(),
                cwd.as_ptr(),
                &si,
                &mut pi,
            )
        };
        if ok == 0 {
            log!("carbon shell: cannot start {}", path.display());
        } else {
            log!("carbon shell: started {name} (pid {})", pi.dwProcessId);
            unsafe {
                CloseHandle(pi.hProcess);
                CloseHandle(pi.hThread);
            }
        }
    }
}
