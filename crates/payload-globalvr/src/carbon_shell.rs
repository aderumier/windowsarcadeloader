//! The cabinet frontend of Need for Speed Carbon, NASCAR Team Racing and PGA TOUR Golf Team
//! Challenge (the same Anark player `AMPlayer.exe` running `shell.am`): the cabinet's monitor
//! programs it talks to, started with it (unless already running: Team Challenge's game loads
//! the payload too), and its scripts' cabinet-only parts.
//!
//! On the cabinet `GVRBoot.exe` starts the Hercules monitors with the frontend. Its
//! `HerculesPlugIn.dll` needs the coin monitor (`GVRCoinMonitor.exe`: coins and credits, shared
//! memory `GVRCoinMonitor_map`) and the volume monitor (`GVRVolumeMonitor.exe`: the attract reads
//! the volume every few frames, each read waited 1 s for an answer without it: the attract drew
//! a frame a second). Both read the USBIO board through `USBIOExtreme.dll`: the payload,
//! installed next to them, feeds them the virtual sticks' coins. Loaded in the frontend, the
//! payload starts them (`WAL_GLOBALVR_CARBON_MONITORS`: the monitors' folder, relative to the
//! frontend's executable). Justice League: Heroes United's `JLA.exe` takes its coins from the
//! same coin monitor: started by the payload loaded in it the same way.
//!
//! The player's script compilations (its SpiderMonkey's `JS_CompileUC*` / `JS_EvaluateUC*`,
//! called directly) get a copy of the script with these edits ([`EDITS`]):
//! * the startup checks (`Shell.am`'s script, "Please Wait") wait 45 s (Carbon) / 15 s (NASCAR)
//!   before reading the I/O board (`ioBoardDelay = presentation.realTime + 45.0`: the cabinet's
//!   USB board coming up), the payload's board answers at once: 1 s;
//! * NASCAR's startup sets the screen mode with the NVIDIA control panel (`rundll32 nvcpl.dll`)
//!   and waits for it: no wait (it fails);
//! * NASCAR's attract reads the linked cabinets' packets every frame, none without the linking
//!   plug-in: a script error each frame (its packet checked first);
//! * NASCAR Racing 1.1's scenes send their state to the linked cabinets (`SendPacket`, a method
//!   of the linking plug-in, not loaded): a call no plug-in answers ends the script's whole
//!   frame (the driver selection never showed its car): not sent.
//!
//! The presentation thread's message wait never blocks ([`render_get_message`]): a message it
//! was told about could vanish before it took it, and the frontend froze.
//!
//! The decompressed `.am` text itself stays untouched: the player checks it (an edited script
//! did not run).
//!
//! `WAL_GLOBALVR_SHELL_PATCHES`: code patches of the frontend, in the `WAL_PATCHES` format
//! (`<module>!<rva>:<hex>` for its plug-ins).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageA, WM_QUIT};

const MONITORS: &[&str] = &["GVRCoinMonitor.exe", "GVRVolumeMonitor.exe"];

/// The player's script compilations (SpiderMonkey, UTF-16 source), as image offsets: each
/// function and its calls, per player build.
struct Build {
    name: &'static str,
    compile_script: usize,
    compile_script_calls: [usize; 7],
    compile_function: usize,
    compile_function_calls: [usize; 3],
    evaluate_script: usize,
    evaluate_script_calls: [usize; 3],
    /// The presentation thread's `call [GetMessageA]` (6 bytes), see [`render_get_message`].
    render_get_message: Option<usize>,
}

const BUILDS: &[Build] = &[
    Build {
        name: "Anark Client 4.0 (Need for Speed Carbon, NASCAR Team Racing)",
        compile_script: 0x26c080,
        compile_script_calls: [0x26c5b6, 0x26de9b, 0x26dee0, 0x26ea58, 0x2733bf, 0x27c336, 0x2ac69f],
        compile_function: 0x26c250,
        compile_function_calls: [0x26df9a, 0x26dfef, 0x26ebc7],
        evaluate_script: 0x26c590,
        evaluate_script_calls: [0x26e180, 0x26e1c5, 0x26ec1d],
        render_get_message: Some(0x21490),
    },
    Build {
        name: "Anark Client 3.0 (NASCAR Racing 1.1)",
        compile_script: 0x1cfe40,
        compile_script_calls: [0x1d0376, 0x1d1c5b, 0x1d1ca0, 0x1d2818, 0x1d717f, 0x1e00f6, 0x21045f],
        compile_function: 0x1d0010,
        compile_function_calls: [0x1d1d5a, 0x1d1daf, 0x1d2987],
        evaluate_script: 0x1d0350,
        evaluate_script_calls: [0x1d1f40, 0x1d1f85, 0x1d29dd],
        render_get_message: None,
    },
    Build {
        name: "Anark Client 3.0 (PGA TOUR Golf Team Challenge)",
        compile_script: 0x1cce80,
        compile_script_calls: [0x1cd3b6, 0x1cec9b, 0x1cece0, 0x1cf858, 0x1d41bf, 0x1dd136, 0x20d49f],
        compile_function: 0x1cd050,
        compile_function_calls: [0x1ced9a, 0x1cedef, 0x1cf9c7],
        evaluate_script: 0x1cd390,
        evaluate_script_calls: [0x1cef80, 0x1cefc5, 0x1cfa1d],
        render_get_message: Some(0x21490),
    },
];
/// The scripts' edits: text, replacement (a leading 0 would make the number octal).
const EDITS: &[(&str, &str)] = &[
    ("presentation.realTime + 45.0;", "presentation.realTime + 1.0;"),
    ("presentation.realTime + 15.0;", "presentation.realTime + 1.0;"),
    (" 32 60\", \"\", 0 );", " 32 60\", \"\", 2 );"),
    ("while ( receivedPacket.length > 0 )", "while ( receivedPacket && receivedPacket.length > 0 )"),
    ("Gvr.Call( \"SendPacket\", this.SendString );", ";"),
    ("Gvr.Call( \"SendPacket\", sendString );", ";"),
];

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

/// The profile's own edits, for debugging (`output()` checkpoints, in the player's log with
/// `aklog.rs`): `WAL_GLOBALVR_SCRIPT_EDITS=<text>=><replacement>|||...`.
fn extra_edits() -> &'static [(String, String)] {
    static EXTRA: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    EXTRA.get_or_init(|| {
        std::env::var("WAL_GLOBALVR_SCRIPT_EDITS")
            .unwrap_or_default()
            .split("|||")
            .filter_map(|e| e.split_once("=>").map(|(a, b)| (a.to_string(), b.to_string())))
            .collect()
    })
}

/// `text` with every [`EDITS`] text replaced, or None without any.
fn edited(text: &[u16]) -> Option<Vec<u16>> {
    let mut out: Option<Vec<u16>> = None;
    let extra = extra_edits().iter().map(|(a, b)| (a.as_str(), b.as_str()));
    for (from, to) in EDITS.iter().copied().chain(extra) {
        let from: Vec<u16> = from.encode_utf16().collect();
        let to: Vec<u16> = to.encode_utf16().collect();
        let current = out.as_deref().unwrap_or(text);
        if !current.windows(from.len()).any(|w| w == from) {
            continue;
        }
        let mut new = Vec::with_capacity(current.len() + to.len());
        let mut i = 0;
        while i < current.len() {
            if current[i..].starts_with(&from) {
                new.extend_from_slice(&to);
                i += from.len();
            } else {
                new.push(current[i]);
                i += 1;
            }
        }
        log!("carbon shell: script edit {:?}", String::from_utf16_lossy(&to));
        out = Some(new);
    }
    out
}

/// The script with its edits (a copy kept for good: the engine may refer to its source), else
/// the script itself.
fn script(chars: *const u16, length: usize) -> (*const u16, usize) {
    if chars.is_null() || length == 0 {
        return (chars, length);
    }
    match edited(unsafe { std::slice::from_raw_parts(chars, length) }) {
        Some(copy) => {
            let length = copy.len();
            (Box::leak(copy.into_boxed_slice()).as_ptr(), length)
        }
        None => (chars, length),
    }
}

unsafe extern "C" fn compile_script(cx: P, obj: P, pr: P, chars: *const u16, length: usize, file: P, line: u32) -> P {
    let f: CompileScript = unsafe { std::mem::transmute(ORIG_COMPILE_SCRIPT.load(Ordering::Relaxed)) };
    let (chars, length) = script(chars, length);
    unsafe { f(cx, obj, pr, chars, length, file, line) }
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
    let (chars, length) = script(chars, length);
    unsafe { f(cx, obj, pr, name, nargs, argnames, chars, length, file, line) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn evaluate_script(cx: P, obj: P, pr: P, chars: *const u16, length: usize, file: P, line: u32, rval: P) -> i32 {
    let f: EvaluateScript = unsafe { std::mem::transmute(ORIG_EVALUATE_SCRIPT.load(Ordering::Relaxed)) };
    let (chars, length) = script(chars, length);
    unsafe { f(cx, obj, pr, chars, length, file, line, rval) }
}

/// The presentation thread's loop renders a frame while `PeekMessageA` (`PM_NOREMOVE`) finds no
/// message, else takes it with `GetMessageA`, which waits. A message Peek saw can be gone when
/// Get looks (one for a window destroyed in between: the attract's slides, coins coming fast at
/// its start): Get then waited for good and the frontend stood still on its last frame. Here
/// the message is taken without waiting, else an empty one (`WM_NULL`): the loop renders on.
/// The player's own `PeekMessageA` import is used (the keyboard filter's, `keyboard.rs`).
unsafe extern "system" fn render_get_message(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32 {
    type Peek = unsafe extern "system" fn(*mut MSG, HWND, u32, u32, u32) -> i32;
    let peek: Peek = match PEEK_SLOT.load(Ordering::Relaxed) {
        0 => PeekMessageA,
        slot => unsafe { std::mem::transmute(*(slot as *const usize)) },
    };
    if unsafe { peek(msg, hwnd, min, max, PM_REMOVE) } != 0 {
        return (unsafe { (*msg).message } != WM_QUIT) as i32;
    }
    unsafe { *msg = std::mem::zeroed() };
    1
}

/// The player's `PeekMessageA` import slot (0: none found).
static PEEK_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Rewrites the 6-byte `call [GetMessageA]` at `site` (image offset) to `call
/// render_get_message; nop`, if it is there.
fn hook_render_get_message(base: usize, site: usize) {
    let at = base + site;
    let code = unsafe { std::slice::from_raw_parts(at as *const u8, 6) };
    let slot = u32::from_le_bytes(code[2..6].try_into().unwrap()) as usize;
    let get_message = unsafe { GetProcAddress(GetModuleHandleA(c"user32.dll".as_ptr().cast()), c"GetMessageA".as_ptr().cast()) };
    if code[..2] != [0xFF, 0x15] || get_message.is_none_or(|f| unsafe { *(slot as *const usize) } != f as usize) {
        log!("carbon shell: presentation loop's GetMessageA not found, left waiting");
        return;
    }
    PEEK_SLOT.store(unsafe { iat::hook_addr("user32.dll", "PeekMessageA") }.unwrap_or(0), Ordering::Relaxed);
    let rel = (render_get_message as *const () as usize).wrapping_sub(at + 5) as i32;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 6, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::write_unaligned(at as *mut u8, 0xE8);
        std::ptr::write_unaligned((at + 1) as *mut i32, rel);
        std::ptr::write_unaligned((at + 5) as *mut u8, 0x90);
        VirtualProtect(at as *const c_void, 6, old, &mut old);
    }
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
    let size = crate::exe_size();
    let Some(b) = BUILDS.iter().find(|b| {
        let sites = b.compile_script_calls.iter().chain(&b.compile_function_calls).chain(&b.evaluate_script_calls);
        sites.chain([b.compile_script, b.compile_function, b.evaluate_script].iter()).all(|&at| at + 5 <= size)
            && calls_found(base, b.compile_script, &b.compile_script_calls)
            && calls_found(base, b.compile_function, &b.compile_function_calls)
            && calls_found(base, b.evaluate_script, &b.evaluate_script_calls)
    }) else {
        log!("carbon shell: script compilation calls not found, scripts not edited");
        return;
    };
    log!("carbon shell: player {}", b.name);
    ORIG_COMPILE_SCRIPT.store(base + b.compile_script, Ordering::Relaxed);
    ORIG_COMPILE_FUNCTION.store(base + b.compile_function, Ordering::Relaxed);
    ORIG_EVALUATE_SCRIPT.store(base + b.evaluate_script, Ordering::Relaxed);
    repoint(base, &b.compile_script_calls, compile_script as *const () as usize);
    repoint(base, &b.compile_function_calls, compile_function as *const () as usize);
    repoint(base, &b.evaluate_script_calls, evaluate_script as *const () as usize);
    if let Some(site) = b.render_get_message.filter(|&s| s + 6 <= size) {
        hook_render_get_message(base, site);
    }
}

pub(crate) fn init() {
    let Ok(dir) = std::env::var("WAL_GLOBALVR_CARBON_MONITORS") else { return };
    let exe = crate::frontend_fit::exe_name();
    // the monitors load the payload too (their USBIOExtreme.dll)
    if MONITORS.iter().any(|m| m.eq_ignore_ascii_case(&exe)) {
        return;
    }
    if exe == "amplayer.exe" {
        hook_scripts();
        if let Ok(spec) = std::env::var("WAL_GLOBALVR_SHELL_PATCHES") {
            wal_payload_common::patches::apply_list(&spec);
        }
    }
    let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else {
        return;
    };
    let dir = exe_dir.join(dir);
    // not from DllMain (the loader lock): a thread, run once the frontend's plug-in is loaded
    std::thread::spawn(move || start(&dir));
}

fn start(dir: &std::path::Path) {
    // already running (started with the frontend, which starts the game: PGA TOUR Golf Team
    // Challenge's engine loads the payload too)
    let map = unsafe { windows_sys::Win32::System::Memory::OpenFileMappingA(windows_sys::Win32::System::Memory::FILE_MAP_READ, 0, c"GVRCoinMonitor_map".as_ptr().cast()) };
    if !map.is_null() {
        unsafe { CloseHandle(map) };
        log!("carbon shell: the coin monitor is running");
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn script_edits() {
        let text = utf16("a = presentation.realTime + 15.0;\nwhile ( receivedPacket.length > 0 ) x;");
        let out = String::from_utf16(&edited(&text).unwrap()).unwrap();
        assert_eq!(out, "a = presentation.realTime + 1.0;\nwhile ( receivedPacket && receivedPacket.length > 0 ) x;");
        assert!(edited(&utf16("nothing here")).is_none());
    }
}
