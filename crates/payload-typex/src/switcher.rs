//! The cabinet's program switcher (`WAL_TYPEX_SWITCHER=<exe name>`). Harikiri Professional
//! Baseball changes programs through `switch.exe`: the game runs `switch.exe test.exe -s` on the
//! test switch, the switcher closes the game's window, waits for it to be gone, starts
//! `test.exe -s` and exits; the test menu goes back the same way (`switch.exe game.exe -s`).
//!
//! The launcher's game is the program it started: when it exits the run ends. So the program
//! asking for the switch also asks the launcher to start the next program when it exits
//! (`exe=<program> <args>`, through `wal-loader` with the payload), and starts the switcher with
//! the payload too; in the switcher, starting the next program is answered as done (the launcher
//! starts it). Its other duties (closing the current program) stay its own.
//!
//! The test menu's EXIT restarts the cabinet (`ExitWindowsEx`): the launcher starts the game
//! again instead.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameA;

static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);
static PAYLOAD: AtomicUsize = AtomicUsize::new(0);

type CreateProcessA = unsafe extern "system" fn(*const u8, *mut u8, usize, usize, i32, u32, usize, *const u8, usize, usize) -> i32;

/// The switcher's executable name (`WAL_TYPEX_SWITCHER`).
fn switcher() -> Option<String> {
    std::env::var("WAL_TYPEX_SWITCHER").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Whether this process is the switcher.
pub fn is_switcher() -> bool {
    switcher().is_some_and(|name| wal_payload_common::patches::exe_name().eq_ignore_ascii_case(&name))
}

/// In every program of the game (`WAL_TYPEX_SWITCHER` set): their program starts handled.
pub fn init(payload: HINSTANCE) {
    if switcher().is_none() {
        return;
    }
    PAYLOAD.store(payload as usize, Ordering::Relaxed);
    match unsafe { iat::hook("kernel32.dll", "CreateProcessA", create_process as *const () as usize) } {
        Some(o) => {
            ORIG_CREATE_PROCESS.store(o, Ordering::Relaxed);
            log!("switcher: program switches through the launcher");
        }
        None => log!("switcher: no CreateProcessA import"),
    }
    let _ = unsafe { iat::hook("user32.dll", "ExitWindowsEx", exit_windows as *const () as usize) };
}

/// `ExitWindowsEx`: the cabinet's restart (the test menu's EXIT): the game started again by the
/// launcher, this program ends.
unsafe extern "system" fn exit_windows(flags: u32, _reason: u32) -> i32 {
    log!("switcher: cabinet restart ({flags:#x}): the game again (launcher)");
    wal_payload_common::request_relaunch("");
    // the request sent before the process goes
    std::thread::sleep(std::time::Duration::from_millis(200));
    unsafe { windows_sys::Win32::System::Threading::ExitProcess(0) }
}

/// A Windows path of a module.
fn module_path(module: HINSTANCE) -> Option<String> {
    let mut buf = [0u8; 520];
    let n = unsafe { GetModuleFileNameA(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf8_lossy(&buf[..n]).into_owned())
}

fn c_str(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned()
}

/// A command line's program (its file name) and arguments.
fn split(line: &str) -> (String, String) {
    let line = line.trim();
    let (program, args) = match line.strip_prefix('"').and_then(|l| l.split_once('"')) {
        Some((p, rest)) => (p, rest),
        None => line.split_once(' ').unwrap_or((line, "")),
    };
    (program.rsplit(['\\', '/']).next().unwrap_or(program).to_string(), args.trim().to_string())
}

/// Whether `name` is a program of the game folder.
fn in_game_folder(name: &str) -> Option<String> {
    let folder = module_path(std::ptr::null_mut())?.rsplit_once('\\')?.0.to_string();
    std::path::Path::new(&format!("{folder}\\{name}")).exists().then_some(folder)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_process(
    app: *const u8,
    cmd: *mut u8,
    pa: usize,
    ta: usize,
    inherit: i32,
    flags: u32,
    env: usize,
    dir: *const u8,
    si: usize,
    pi: usize,
) -> i32 {
    let orig: CreateProcessA = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    let (app_s, cmd_s) = (c_str(app), c_str(cmd as *const u8));
    let line = if app_s.is_empty() { cmd_s.clone() } else { format!("\"{app_s}\" {}", split(&cmd_s).1) };
    let (program, args) = split(&line);
    let as_asked = || {
        log!("switcher: starts {line:?} as asked");
        unsafe { orig(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }
    };
    let Some(folder) = in_game_folder(&program) else { return as_asked() };
    let switcher = switcher().unwrap_or_default();
    if is_switcher() {
        // the next program: the launcher starts it when the current one has exited
        log!("switcher: {program} {args} started by the launcher");
        unsafe { fake_process(pi) };
        return 1;
    }
    if !program.eq_ignore_ascii_case(&switcher) {
        return as_asked();
    }
    // `switch.exe <next program> <args>`: the launcher starts the next program after this one
    let next = args.trim();
    if next.is_empty() {
        return as_asked();
    }
    log!("switcher: then {next} (launcher)");
    wal_payload_common::request_relaunch(&format!("exe={next}"));
    // the switcher with the payload (it closes this program)
    let Some(payload) = module_path(PAYLOAD.load(Ordering::Relaxed) as HINSTANCE) else { return as_asked() };
    let command = format!("\"{folder}\\wal-loader.exe\" \"{payload}\" \"{folder}\\{program}\" {args}");
    log!("switcher: {command}");
    let mut command = std::ffi::CString::new(command).unwrap_or_default().into_bytes_with_nul();
    let folder = std::ffi::CString::new(folder).unwrap_or_default();
    // the loader is a console program: no console window
    let flags = flags | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    let ok = unsafe { orig(std::ptr::null(), command.as_mut_ptr(), pa, ta, inherit, flags, env, folder.as_ptr().cast(), si, pi) };
    if ok == 0 {
        log!("switcher: {program} did not start (error {})", unsafe { windows_sys::Win32::Foundation::GetLastError() });
    }
    ok
}

/// A PROCESS_INFORMATION of a program not started: handles the caller can wait on and close.
unsafe fn fake_process(pi: usize) {
    use windows_sys::Win32::System::Threading::CreateEventA;
    if pi == 0 {
        return;
    }
    let pi = pi as *mut usize;
    unsafe {
        *pi = CreateEventA(std::ptr::null(), 1, 0, std::ptr::null()) as usize;
        *pi.add(1) = CreateEventA(std::ptr::null(), 1, 0, std::ptr::null()) as usize;
        *(pi.add(2) as *mut u32) = 0x5741;
        *(pi.add(3) as *mut u32) = 0x5742;
    }
}
