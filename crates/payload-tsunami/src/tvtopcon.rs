//! The cabinet's software (`Tsunami/`): its Windows shell `TVTSHELL.EXE` (runs the operator
//! console `tvtopcon.exe`, then the selected game's `cmdline`, and again; on an error it reboots
//! the cabinet), the console (the test menu, and at each start the selection of the game and its
//! display from `HKLM\Software\Tsunami\Apps`) and the attract / game launcher
//! `Launch/Launch.exe` (`Select.exe`).
//!
//! * The game the dump holds is registered there (its cabinet `launch.reg` values, from the
//!   profile's files or the dump, its `c:\games\<name>` paths made the run directory's), else
//!   "No games are available". The console's last error is cleared and the cabinet's hardware
//!   version set (none: an unconfigured cabinet, the operator menu opens at each start). Its
//!   motion seat driver is the cabinet's null one (`MotionLib` = `mdi_null.dll`): the default
//!   `motionLib.dll` polled the seat's I/O board (`cbw32.dll`) for 13 s at each start. Its
//!   start-up 2 s sleep is cut and its boot screen's wait for the operator ("Press Test to
//!   enter Operator Console.") is 1 s instead of 5 (its 1.5 s minimum kept).
//! * Its adapter walk (the cabinet's network address, `GetAdaptersInfo`) only moved to the next
//!   adapter after an Ethernet one: on a machine whose first adapter is another kind (Wi-Fi, a
//!   VPN) it spun forever. Made to skip the other kinds.
//! * Its per-game license (a key for the cabinet's machine id, `Tsunami\license.bin`): every
//!   game licensed for good, else "<game> is not licensed".
//! * The programs they start from the dump (the console, the games) go through `wal-loader`
//!   with this payload (their cabinet objects emulated), as the Global VR frontends' games. The
//!   shell's reboot (`ExitWindowsEx`, after `C:\Tsunami\restart.pif`) ends it instead.
//! * The console sets 640x480 16-bit and then the selected game's colour depth (`bitDepth`, 16
//!   for Re-Volt) before the shell starts it: under Wine the mode stays after it ends, and
//!   Re-Volt's car bodies were invisible on the 16-bit desktop. Its boot screen (20 s for the
//!   operator to press Enter, nothing drawn) keeps the desktop's mode, its window black; the
//!   operator console (`RunOpCons` set) gets its 640x480 mode, the desktop's restored when it
//!   ends (the game sets its own fullscreen mode).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
};

static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SHELL_EXECUTE: AtomicUsize = AtomicUsize::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The executable's full path.
fn exe_path() -> String {
    let mut buf = [0u16; 520];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// The cabinet program this process is (lowercase file name), if one.
pub fn cabinet_program() -> Option<&'static str> {
    let exe = exe_path().to_ascii_lowercase();
    ["tvtshell.exe", "tvtopcon.exe", "launch.exe", "select.exe"].into_iter().find(|p| exe.ends_with(&format!("\\{p}")))
}

/// The adapter walk at 0x41f914: `jne 0x41f933` (the next test, the same adapter again) after a
/// non-Ethernet adapter made `jne 0x41f931` (`mov eax, [eax]`: the next adapter).
unsafe fn fix_adapter_walk() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let at = (base + 0x1f914) as *mut u8;
    let expected: [u8; 9] = [0x83, 0xb8, 0x90, 0x01, 0x00, 0x00, 0x06, 0x75, 0x16];
    if unsafe { std::slice::from_raw_parts(at, 9) } != expected {
        log!("tvtopcon: adapter walk signature mismatch, not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at.add(8).cast(), 1, PAGE_EXECUTE_READWRITE, &mut old);
        *at.add(8) = 0x14;
        VirtualProtect(at.add(8).cast(), 1, old, &mut old);
    }
    log!("tvtopcon: adapter walk patched (non-Ethernet adapters skipped)");
}

/// The license check of a game (`bool __thiscall check()` at 0x415c20; licensed: 1, its time
/// left at `[this+0x78]` negative for a permanent license): licensed for good.
unsafe fn license_games() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let at = (base + 0x15c20) as *mut u8;
    let expected: [u8; 13] = [0x81, 0xec, 0x00, 0x01, 0x00, 0x00, 0x53, 0x56, 0x57, 0x8b, 0xf9, 0x33, 0xdb];
    if unsafe { std::slice::from_raw_parts(at, 13) } != expected {
        log!("tvtopcon: license check signature mismatch, not patched");
        return;
    }
    // mov dword [ecx+0x78], -1; mov eax, 1; ret
    let code: [u8; 13] = [0xc7, 0x41, 0x78, 0xff, 0xff, 0xff, 0xff, 0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3];
    let mut old = 0;
    unsafe {
        VirtualProtect(at.cast(), code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at, code.len());
        VirtualProtect(at.cast(), code.len(), old, &mut old);
    }
    log!("tvtopcon: games licensed");
}

/// The console's start-up waits: `Sleep(2000)` (0x418cb3, `push 0x7d0`) made `Sleep(0)`, the
/// operator's 5 s on the boot screen (0x419c09, `cmp eax, 0x1388`) made 1 s.
unsafe fn shorten_boot() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let patches: [(usize, [u8; 5], [u8; 5]); 2] = [
        (0x18cb3, [0x68, 0xd0, 0x07, 0x00, 0x00], [0x68, 0x00, 0x00, 0x00, 0x00]),
        (0x19c09, [0x3d, 0x88, 0x13, 0x00, 0x00], [0x3d, 0xe8, 0x03, 0x00, 0x00]),
    ];
    for (rva, from, to) in patches {
        let at = (base + rva) as *mut u8;
        if unsafe { std::slice::from_raw_parts(at, 5) } != from {
            log!("tvtopcon: boot wait signature mismatch at +{rva:#x}, not shortened");
            continue;
        }
        let mut old = 0;
        unsafe {
            VirtualProtect(at.cast(), 5, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(to.as_ptr(), at, 5);
            VirtualProtect(at.cast(), 5, old, &mut old);
        }
    }
    log!("tvtopcon: boot waits shortened (operator 1 s)");
}

/// The value of `"name"="value"` lines of a `.reg` file section (REGEDIT4, `\\` escaped).
fn reg_values(text: &str, section: &str) -> Vec<(String, RegValue)> {
    let mut values = Vec::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line.trim_matches(['[', ']']).eq_ignore_ascii_case(section);
            continue;
        }
        if !inside {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else { continue };
        let name = name.trim().trim_matches('"').to_string();
        if name.is_empty() || name == "@" {
            continue;
        }
        let value = value.trim();
        if let Some(s) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            values.push((name, RegValue::Str(s.replace("\\\\", "\\"))));
        } else if let Some(d) = value.strip_prefix("dword:").and_then(|d| u32::from_str_radix(d, 16).ok()) {
            values.push((name, RegValue::Dword(d)));
        }
    }
    values
}

enum RegValue {
    Str(String),
    Dword(u32),
}

/// The dump's game in `Software\Tsunami\Apps\<name>`: the `launch.reg` beside the dump root's
/// executables, its folder (`path`, `c:\games\ReVolt` on the cabinet) replaced by `root`.
fn register_game(root: &str) {
    // the profile's copy beside the cabinet programs, else the dump's
    let exe = exe_path();
    let exe_dir = exe.rsplit_once('\\').map_or("", |(d, _)| d).to_string();
    let found = [format!("{exe_dir}\\launch.reg"), format!("{root}\\Tsunami\\launch.reg"), format!("{root}\\launch.reg")]
        .into_iter()
        .find_map(|f| std::fs::read(&f).ok().map(|b| (f, String::from_utf8_lossy(&b).into_owned())));
    let Some((file, text)) = found else {
        log!("tvtopcon: no launch.reg, no game registered");
        return;
    };
    let section = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('[') && !l.starts_with("[-") && l.to_ascii_lowercase().starts_with("[hkey_local_machine\\software\\tsunami\\apps\\") && l.matches('\\').count() == 4)
        .map(|l| l.trim_matches(['[', ']']).to_string());
    let Some(section) = section else {
        log!("tvtopcon: {file}: no Software\\Tsunami\\Apps\\<game> section");
        return;
    };
    let values = reg_values(&text, &section);
    let old_path = values.iter().find_map(|(n, v)| match v {
        RegValue::Str(s) if n.eq_ignore_ascii_case("path") => Some(s.clone()),
        _ => None,
    });
    let game = section.rsplit('\\').next().unwrap_or("Game").to_string();
    let key_path = wide(&format!("Software\\Tsunami\\Apps\\{game}"));
    let mut key: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, key_path.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut())
    };
    if r != 0 {
        log!("tvtopcon: cannot create Software\\Tsunami\\Apps\\{game}: error {r}");
        return;
    }
    for (name, value) in &values {
        let n = wide(name);
        let r = match value {
            RegValue::Str(s) => {
                // the cabinet's folder (any case) made the run directory
                let s = match &old_path {
                    Some(old) if s.to_ascii_lowercase().starts_with(&old.to_ascii_lowercase()) => format!("{root}{}", &s[old.len()..]),
                    _ => s.clone(),
                };
                let w = wide(&s);
                unsafe { RegSetValueExW(key, n.as_ptr(), 0, windows_sys::Win32::System::Registry::REG_SZ, w.as_ptr().cast(), (w.len() * 2) as u32) }
            }
            RegValue::Dword(d) => unsafe { RegSetValueExW(key, n.as_ptr(), 0, REG_DWORD, (d as *const u32).cast(), 4) },
        };
        if r != 0 {
            log!("tvtopcon: {game} {name}: error {r}");
        }
    }
    unsafe { RegCloseKey(key) };
    log!("tvtopcon: {game} registered in Software\\Tsunami\\Apps ({} values, folder {root})", values.len());
}

fn c_str(p: *const u8) -> String {
    if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() }
}

/// The program of a command line (quoted or up to the first space) and its arguments.
fn split_command(cmd: &str) -> (String, String) {
    let cmd = cmd.trim_start();
    match cmd.strip_prefix('"') {
        Some(rest) => match rest.split_once('"') {
            Some((exe, args)) => (exe.to_string(), args.trim().to_string()),
            None => (rest.to_string(), String::new()),
        },
        None => match cmd.split_once(' ') {
            Some((exe, args)) => (exe.to_string(), args.trim().to_string()),
            None => (cmd.to_string(), String::new()),
        },
    }
}

/// `CreateProcessA` of the console: a program of the dump (a game) started through `wal-loader`
/// with this payload, the others (the cabinet's tools) as asked.
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
    type CreateProcessA = unsafe extern "system" fn(*const u8, *mut u8, usize, usize, i32, u32, usize, *const u8, usize, usize) -> i32;
    let orig: CreateProcessA = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    let (app_s, cmd_s, dir_s) = (c_str(app), c_str(cmd as *const u8), c_str(dir));
    log!("tvtopcon: starts {app_s:?} {cmd_s:?} in {dir_s:?}");
    let (exe, args) = if app_s.is_empty() {
        split_command(&cmd_s)
    } else {
        let (first, rest) = split_command(&cmd_s);
        (app_s.clone(), if first.eq_ignore_ascii_case(&app_s) { rest } else { cmd_s.clone() })
    };
    // a relative program (the shell's "TVTOPCON.EXE"): from the given folder, else the current one
    let exe = full_path(&exe, &dir_s);
    let root = game_root();
    let (Some(payload), true) = (crate::dll_path(), exe.to_ascii_lowercase().starts_with(&format!("{}\\", root.to_ascii_lowercase()))) else {
        return unsafe { orig(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) };
    };
    let loader = payload.rsplit_once('\\').map_or("wal-loader.exe".to_string(), |(d, _)| format!("{d}\\wal-loader.exe"));
    let command = format!("\"{loader}\" \"{payload}\" \"{exe}\" {args}");
    let folder = if dir_s.is_empty() { exe.rsplit_once('\\').map_or(String::new(), |(d, _)| d.to_string()) } else { dir_s };
    log!("tvtopcon: {command} (in {folder})");
    let mut command = std::ffi::CString::new(command).unwrap_or_default().into_bytes_with_nul();
    let folder = std::ffi::CString::new(folder).unwrap_or_default();
    // the loader is a console program: no console window over the console's screen
    let flags = flags | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    let ok = unsafe { orig(std::ptr::null(), command.as_mut_ptr(), pa, ta, inherit, flags, env, folder.as_ptr().cast(), si, pi) };
    if ok == 0 {
        log!("tvtopcon: the game did not start (error {})", unsafe { windows_sys::Win32::Foundation::GetLastError() });
    }
    ok
}

/// `path` made absolute: from `dir`, else the current folder.
fn full_path(path: &str, dir: &str) -> String {
    let bytes = path.as_bytes();
    if path.starts_with("\\\\") || (bytes.len() > 2 && bytes[1] == b':') {
        return path.to_string();
    }
    let base = if dir.is_empty() {
        let mut buf = [0u8; 520];
        let n = unsafe { windows_sys::Win32::System::Environment::GetCurrentDirectoryA(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        String::from_utf8_lossy(&buf[..n.min(buf.len())]).into_owned()
    } else {
        dir.to_string()
    };
    format!("{}\\{path}", base.trim_end_matches('\\'))
}

/// The dump root: the folder holding the cabinet's `Tsunami\` folder.
fn game_root() -> String {
    let exe = exe_path();
    match exe.to_ascii_lowercase().rfind("\\tsunami\\") {
        Some(i) => exe[..i].to_string(),
        None => exe.rsplit_once('\\').map_or("", |(d, _)| d).to_string(),
    }
}

/// The shell's `ShellExecuteA`: its reboot script (`C:\Tsunami\restart.pif`) not run.
unsafe extern "system" fn shell_execute(hwnd: usize, verb: *const u8, file: *const u8, params: *const u8, dir: *const u8, show: i32) -> usize {
    let file_s = c_str(file);
    log!("tvtopcon: shell executes {file_s:?} {:?}", c_str(params));
    if file_s.to_ascii_lowercase().ends_with("restart.pif") {
        return 33;
    }
    type ShellExecuteA = unsafe extern "system" fn(usize, *const u8, *const u8, *const u8, *const u8, i32) -> usize;
    let orig: ShellExecuteA = unsafe { std::mem::transmute(ORIG_SHELL_EXECUTE.load(Ordering::Relaxed)) };
    unsafe { orig(hwnd, verb, file, params, dir, show) }
}

static ORIG_CHANGE_DISPLAY: AtomicUsize = AtomicUsize::new(0);
static MODE_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static CONSOLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ORIG_REGISTER_CLASS: AtomicUsize = AtomicUsize::new(0);

/// The operator console opens at this start (`Software\Tsunami` `RunOpCons` set).
fn console_requested() -> bool {
    use windows_sys::Win32::System::Registry::{KEY_QUERY_VALUE, RegOpenKeyExW, RegQueryValueExW};
    let path = wide("Software\\Tsunami");
    let name = wide("RunOpCons");
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) } != 0 {
        return false;
    }
    let (mut value, mut size) = (0u32, 4u32);
    let ok = unsafe { RegQueryValueExW(key, name.as_ptr(), std::ptr::null(), std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size) } == 0;
    unsafe { RegCloseKey(key) };
    ok && value != 0
}

/// The cabinet programs' window classes with a black background (the console's boot screen
/// draws nothing: a white box on the black screen).
unsafe extern "system" fn register_class(class: *mut u8) -> u16 {
    if !class.is_null() {
        // WNDCLASSA.hbrBackground at 28
        let brush = unsafe { windows_sys::Win32::Graphics::Gdi::GetStockObject(windows_sys::Win32::Graphics::Gdi::BLACK_BRUSH) };
        unsafe { (class.add(28) as *mut usize).write_unaligned(brush as usize) };
    }
    type RegisterClassA = unsafe extern "system" fn(*mut u8) -> u16;
    let orig: RegisterClassA = unsafe { std::mem::transmute(ORIG_REGISTER_CLASS.load(Ordering::Relaxed)) };
    unsafe { orig(class) }
}

/// The cabinet programs' display mode changes: the operator console's made (recorded to be
/// undone when it ends), the others answered as done without one.
unsafe extern "system" fn change_display_settings(mode: *const u8, flags: u32) -> i32 {
    // DEVMODEA: dmBitsPerPel at 104, dmPelsWidth / dmPelsHeight at 108 / 112
    let (bits, w, h) = if mode.is_null() {
        (0, 0, 0)
    } else {
        unsafe { ((mode.add(104) as *const u32).read_unaligned(), (mode.add(108) as *const u32).read_unaligned(), (mode.add(112) as *const u32).read_unaligned()) }
    };
    if !CONSOLE.load(Ordering::Relaxed) {
        log!("tvtopcon: display mode {w}x{h}, {bits} bits (flags {flags:#x}) not made (not the operator console)");
        return 0;
    }
    type ChangeDisplaySettingsA = unsafe extern "system" fn(*const u8, u32) -> i32;
    let orig: ChangeDisplaySettingsA = unsafe { std::mem::transmute(ORIG_CHANGE_DISPLAY.load(Ordering::Relaxed)) };
    let r = unsafe { orig(mode, flags) };
    if !mode.is_null() {
        MODE_CHANGED.store(true, Ordering::Relaxed);
    }
    log!("tvtopcon: display mode {w}x{h}, {bits} bits (flags {flags:#x}): {r}");
    r
}

/// The desktop's own mode back, when this cabinet program changed it (process detach).
pub fn restore_display() {
    if !MODE_CHANGED.swap(false, Ordering::Relaxed) {
        return;
    }
    type ChangeDisplaySettingsA = unsafe extern "system" fn(*const u8, u32) -> i32;
    let orig: ChangeDisplaySettingsA = unsafe { std::mem::transmute(ORIG_CHANGE_DISPLAY.load(Ordering::Relaxed)) };
    let r = unsafe { orig(std::ptr::null(), 0) };
    log!("tvtopcon: desktop display mode restored: {r}");
}

/// The shell's reboot after an error: the shell ends (its error logged above).
unsafe extern "system" fn exit_windows(flags: u32, reason: u32) -> i32 {
    log!("tvtopcon: the shell reboots the cabinet ({flags:#x}, {reason:#x}): the shell ends");
    unsafe { windows_sys::Win32::System::Threading::ExitProcess(0) }
}

/// The console's last error (`Software\Tsunami` `LastErrorCode` / `LastErrorMessage`, e.g. "No
/// games are available" from a run before the game was registered) cleared: with one the
/// console opens its operator menu to show it instead of running the games.
fn clear_last_error() {
    use windows_sys::Win32::System::Registry::RegDeleteValueW;
    let path = wide("Software\\Tsunami");
    let mut key: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE | windows_sys::Win32::System::Registry::KEY_QUERY_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut())
    };
    if r != 0 {
        return;
    }
    let zero = 0u32;
    let code = wide("LastErrorCode");
    let message = wide("LastErrorMessage");
    unsafe {
        RegSetValueExW(key, code.as_ptr(), 0, REG_DWORD, (&zero as *const u32).cast(), 4);
        RegDeleteValueW(key, message.as_ptr());
    }
    // the cabinet's hardware version (0x438a94): none, the console takes the cabinet for an
    // unconfigured one and opens its operator menu instead of running the games
    let version = wide("HardwareVersion");
    let mut current = 0u32;
    let mut size = 4u32;
    let found = unsafe {
        windows_sys::Win32::System::Registry::RegQueryValueExW(key, version.as_ptr(), std::ptr::null(), std::ptr::null_mut(), (&mut current as *mut u32).cast(), &mut size)
    } == 0;
    if !found || current == 0 {
        let one = 1u32;
        unsafe { RegSetValueExW(key, version.as_ptr(), 0, REG_DWORD, (&one as *const u32).cast(), 4) };
        log!("tvtopcon: cabinet hardware version 1");
    }
    // the motion seat driver: the null one when none is set (the dump has no seat board)
    let motion = wide("MotionLib");
    let mut size = 0u32;
    let set = unsafe {
        windows_sys::Win32::System::Registry::RegQueryValueExW(key, motion.as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), &mut size)
    } == 0
        && size > 2;
    if !set {
        let null = wide("mdi_null.dll");
        unsafe { RegSetValueExW(key, motion.as_ptr(), 0, windows_sys::Win32::System::Registry::REG_SZ, null.as_ptr().cast(), (null.len() * 2) as u32) };
        log!("tvtopcon: motion seat driver mdi_null.dll (no seat board)");
    }
    unsafe { RegCloseKey(key) };
}

pub fn init(program: &str) {
    clear_last_error();
    register_game(&game_root());
    if program == "tvtopcon.exe" {
        unsafe { fix_adapter_walk() };
        unsafe { license_games() };
        unsafe { shorten_boot() };
    }
    match unsafe { iat::hook("kernel32.dll", "CreateProcessA", create_process as *const () as usize) } {
        Some(o) => ORIG_CREATE_PROCESS.store(o, Ordering::Relaxed),
        None => log!("tvtopcon: {program}: no CreateProcessA import, programs started as asked"),
    }
    if let Some(o) = unsafe { iat::hook("shell32.dll", "ShellExecuteA", shell_execute as *const () as usize) } {
        ORIG_SHELL_EXECUTE.store(o, Ordering::Relaxed);
    }
    let _ = unsafe { iat::hook("user32.dll", "ExitWindowsEx", exit_windows as *const () as usize) };
    CONSOLE.store(program == "tvtopcon.exe" && console_requested(), Ordering::Relaxed);
    if let Some(o) = unsafe { iat::hook("user32.dll", "ChangeDisplaySettingsA", change_display_settings as *const () as usize) } {
        ORIG_CHANGE_DISPLAY.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { iat::hook("user32.dll", "RegisterClassA", register_class as *const () as usize) } {
        ORIG_REGISTER_CLASS.store(o, Ordering::Relaxed);
    }
    log!("tvtopcon: {program} (cabinet software, dump root {})", game_root());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg_section_values() {
        let text = "REGEDIT4\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Tsunami\\Apps\\ReVolt]\r\n\"path\"=\"c:\\\\games\\\\ReVolt\"\r\n\"bitDepth\"=dword:00000010\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Tsunami\\Apps\\ReVolt\\Options]\r\n\"x\"=\"y\"\r\n";
        let values = reg_values(text, "HKEY_LOCAL_MACHINE\\SOFTWARE\\Tsunami\\Apps\\ReVolt");
        assert_eq!(values.len(), 2);
        assert!(matches!(&values[0], (n, RegValue::Str(s)) if n == "path" && s == "c:\\games\\ReVolt"));
        assert!(matches!(&values[1], (n, RegValue::Dword(16)) if n == "bitDepth"));
    }

    #[test]
    fn commands() {
        assert_eq!(split_command("c:\\games\\ReVolt\\ReVolt.exe -launchgame"), ("c:\\games\\ReVolt\\ReVolt.exe".into(), "-launchgame".into()));
        assert_eq!(split_command("\"C:\\a b\\x.exe\" -y"), ("C:\\a b\\x.exe".into(), "-y".into()));
    }
}
