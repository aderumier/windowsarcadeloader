//! The cabinet's test menu: the GameShell's (`GameShell.exe`), run in place of the game.
//!
//! On the cabinet the GameShell runs the game and watches the board's TEST button: it closes
//! the game and shows its test menu, then starts the game again. Here the payload plays the
//! shell for the game, so:
//!
//! * In the game, TEST asks the launcher to start the shell program
//!   (`WAL_EUROPAR_TEST_MENU=<path from the game directory>`, `../GameShell/GameShell.exe`,
//!   the payload loaded there by `wal-loader`) and ends the game.
//! * In the shell: its own directory as the working directory (the cabinet's), its security
//!   checks patched (`WAL_EUROPAR_SHELL_PATCHES`), the board with its TEST / SERVICE buttons
//!   and coin counter (`happ.rs`). Its boot ends starting the game: it gets a game that seems
//!   to run and a TEST press, its own way into its test menu. Leaving the menu starts the game
//!   again (or restarts the cabinet): back to the real game.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, log, patches};
use wal_protocol::button;
use windows_sys::Win32::System::Threading::{CreateEventA, GetCurrentProcess, TerminateProcess};

/// The test menu program, from the game's directory (`WAL_EUROPAR_TEST_MENU`).
fn test_menu() -> Option<String> {
    std::env::var("WAL_EUROPAR_TEST_MENU").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// This process is the test menu program (its file name).
pub(crate) fn is_test_menu() -> bool {
    test_menu().is_some_and(|m| {
        let name = m.rsplit(['/', '\\']).next().unwrap_or_default();
        name.eq_ignore_ascii_case(&patches::exe_name())
    })
}

/// The launcher starts `args` (relaunch request) once this process has ended.
fn leave(args: &str) {
    if !wal_payload_common::connected() {
        log!("testmenu: launcher not connected, staying");
        return;
    }
    wal_payload_common::request_relaunch(args);
    // the request reaches the launcher before the process ends
    std::thread::sleep(Duration::from_millis(300));
    unsafe { TerminateProcess(GetCurrentProcess(), 0) };
}

/// In the game: TEST starts the test menu program. A TEST held at start (back from the test
/// menu) is not a press.
pub(crate) fn watch_game() {
    let Some(menu) = test_menu() else { return };
    std::thread::spawn(move || {
        let mut held = true;
        loop {
            std::thread::sleep(Duration::from_millis(20));
            let down = wal_payload_common::input(0).pressed(button::TEST);
            if down && !held {
                log!("testmenu: TEST, starting {menu}");
                leave(&format!("exe={menu}"));
            }
            held = down;
        }
    });
}

fn c_str(p: *const i8) -> String {
    if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

/// The shell starting a program: the game. The first time (after its boot) a game that seems
/// to run (its process handle a never signaled event), then the shell's own TEST press takes
/// it into its test menu; the next time (the test menu left) back to the real game.
unsafe extern "system" fn create_process(
    app: *const i8,
    cmd: *mut i8,
    _pa: usize,
    _ta: usize,
    _inherit: i32,
    _flags: u32,
    _env: usize,
    dir: *const i8,
    _si: usize,
    pi: *mut usize,
) -> i32 {
    static STARTS: AtomicU32 = AtomicU32::new(0);
    if STARTS.fetch_add(1, Ordering::Relaxed) > 0 {
        log!("testmenu: shell starts {:?} {:?} (in {:?}): back to the game", c_str(app), c_str(cmd), c_str(dir));
        leave("");
        return 0;
    }
    log!("testmenu: shell starts the game: a fake one, then TEST");
    unsafe {
        let event = |_| CreateEventA(std::ptr::null(), 1, 0, std::ptr::null()) as usize;
        if !pi.is_null() {
            *pi = event(());
            *pi.add(1) = event(());
            *pi.add(2) = 0x5741;
            *pi.add(3) = 0x5742;
        }
    }
    crate::happ::press_test(Duration::from_millis(1500), Duration::from_millis(500));
    1
}

/// `ExitWindowsEx`: the shell's restart, back to the game.
unsafe extern "system" fn exit_windows(flags: u32, _reason: u32) -> i32 {
    log!("testmenu: shell restarts the cabinet ({flags:#x}): back to the game");
    leave("");
    0
}

/// In the shell program: its patches (`WAL_EUROPAR_SHELL_PATCHES`, the `WAL_PATCHES` format).
pub(crate) fn init_shell() {
    if let Ok(spec) = std::env::var("WAL_EUROPAR_SHELL_PATCHES") {
        patches::apply_list(&spec);
    }
    // the cabinet runs it from its own directory (Resources, Variable, gameshell.ini)
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
        let _ = std::env::set_current_dir(&dir);
        log!("testmenu: shell in {}", dir.display());
    }
    unsafe {
        iat::hook("kernel32.dll", "CreateProcessA", create_process as *const () as usize);
        iat::hook("user32.dll", "ExitWindowsEx", exit_windows as *const () as usize);
    }
    trace::init();
}

/// `WAL_EUROPAR_TRACE=1`: the shell's file, drive, registry and settings calls, logged.
mod trace {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use wal_payload_common::{iat, log};

    use super::c_str;

    static ORIG: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

    fn orig<T>(i: usize) -> T {
        unsafe { std::mem::transmute_copy(&ORIG[i].load(Ordering::Relaxed)) }
    }

    unsafe extern "system" fn create_file(n: *const i8, a: u32, s: u32, sa: usize, d: u32, f: u32, t: usize) -> usize {
        let r = unsafe { orig::<unsafe extern "system" fn(*const i8, u32, u32, usize, u32, u32, usize) -> usize>(0)(n, a, s, sa, d, f, t) };
        log!("trace: CreateFileA({:?}, {a:#x}, disp {d}) -> {r:#x}", c_str(n));
        r
    }
    unsafe extern "system" fn drive_type(n: *const i8) -> u32 {
        let r = unsafe { orig::<unsafe extern "system" fn(*const i8) -> u32>(1)(n) };
        log!("trace: GetDriveTypeA({:?}) -> {r}", c_str(n));
        r
    }
    unsafe extern "system" fn reg_open(k: usize, n: *const i8, o: u32, s: u32, out: usize) -> i32 {
        let r = unsafe { orig::<unsafe extern "system" fn(usize, *const i8, u32, u32, usize) -> i32>(2)(k, n, o, s, out) };
        log!("trace: RegOpenKeyExA({k:#x}, {:?}) -> {r}", c_str(n));
        r
    }
    unsafe extern "system" fn reg_query(k: usize, n: *const i8, res: usize, ty: usize, data: *mut u8, len: *mut u32) -> i32 {
        let r = unsafe { orig::<unsafe extern "system" fn(usize, *const i8, usize, usize, *mut u8, *mut u32) -> i32>(3)(k, n, res, ty, data, len) };
        log!("trace: RegQueryValueExA({:?}) -> {r}", c_str(n));
        r
    }
    unsafe extern "system" fn get_string(s: *const i8, k: *const i8, d: *const i8, out: *mut i8, n: u32, f: *const i8) -> u32 {
        let r = unsafe { orig::<unsafe extern "system" fn(*const i8, *const i8, *const i8, *mut i8, u32, *const i8) -> u32>(4)(s, k, d, out, n, f) };
        log!("trace: GetPrivateProfileStringA([{}] {} in {:?}) -> {:?}", c_str(s), c_str(k), c_str(f), c_str(out));
        r
    }
    unsafe extern "system" fn get_int(s: *const i8, k: *const i8, d: i32, f: *const i8) -> u32 {
        let r = unsafe { orig::<unsafe extern "system" fn(*const i8, *const i8, i32, *const i8) -> u32>(5)(s, k, d, f) };
        log!("trace: GetPrivateProfileIntA([{}] {} in {:?}) -> {r}", c_str(s), c_str(k), c_str(f));
        r
    }
    unsafe extern "system" fn find_first(n: *const i8, data: usize) -> usize {
        let r = unsafe { orig::<unsafe extern "system" fn(*const i8, usize) -> usize>(6)(n, data) };
        log!("trace: FindFirstFileA({:?}) -> {r:#x}", c_str(n));
        r
    }
    unsafe extern "system" fn write_string(s: *const i8, k: *const i8, v: *const i8, f: *const i8) -> i32 {
        let r = unsafe { orig::<unsafe extern "system" fn(*const i8, *const i8, *const i8, *const i8) -> i32>(7)(s, k, v, f) };
        log!("trace: WritePrivateProfileStringA([{}] {} = {:?} in {:?})", c_str(s), c_str(k), c_str(v), c_str(f));
        r
    }

    pub(crate) fn init() {
        if !std::env::var("WAL_EUROPAR_TRACE").is_ok_and(|v| v.trim() == "1") {
            return;
        }
        let hooks: [(&str, &str, usize); 8] = [
            ("kernel32.dll", "CreateFileA", create_file as *const () as usize),
            ("kernel32.dll", "GetDriveTypeA", drive_type as *const () as usize),
            ("advapi32.dll", "RegOpenKeyExA", reg_open as *const () as usize),
            ("advapi32.dll", "RegQueryValueExA", reg_query as *const () as usize),
            ("kernel32.dll", "GetPrivateProfileStringA", get_string as *const () as usize),
            ("kernel32.dll", "GetPrivateProfileIntA", get_int as *const () as usize),
            ("kernel32.dll", "FindFirstFileA", find_first as *const () as usize),
            ("kernel32.dll", "WritePrivateProfileStringA", write_string as *const () as usize),
        ];
        for (i, (dll, name, f)) in hooks.into_iter().enumerate() {
            if let Some(o) = unsafe { iat::hook(dll, name, f) } {
                ORIG[i].store(o, Ordering::Relaxed);
            }
        }
    }
}
