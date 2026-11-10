//! Senninha GP (`interface.exe` the menus, `bin\<track>\seninha.exe` the races, 3D GameStudio
//! engine `acknex.dll`, Direct3D 9, 32-bit): `WAL_ARCADEPC_BOARD=senninha`, x86 build.
//!
//! - **Board.** Every program of the game (menus, races, podium, operator settings) is an engine
//!   whose scripts call the cabinet's input library (`bin\s_input.dll`, an engine plugin: cdecl
//!   functions by name, values in the engine's 22.10 fixed point): the payload is that library
//!   (`payload_install`, the dump's community replacement put aside), loaded by each of them.
//!   `WheelValue` the wheel (-1..1, left positive), `AccelValue` / `BrakeValue` the pedals
//!   (0..1), `ButtonDown` the switches (bit 0 service: a credit, 1 test, 2 camera, 4 item,
//!   5 boost, 6 start, 7 coin: the operator menu's input test). The lamps, tickets, shaker and force feedback functions do nothing.
//! - **Programs.** Each program starts the next one (the engine's `exec`, `CreateProcessA`) and
//!   ends: the payload asks the launcher to start it instead (a relaunch, `exe=` from the game's
//!   root, the folder above `bin\s_input.dll`), so the launcher follows the game from program
//!   to program (the cabinet's `bin\winexec\WinExec.exe`, which only starts the menus again, and
//!   the record holder's webcam photo `bin\recordes\recordcam.exe`, which then starts them: the
//!   menus).
//! - **Free play.** The common `WAL_FREEPLAY` sets the game's own (its operator menu's FREE PLAY:
//!   `bin\config_senninha.ini` `[Coin]` `value=0` credits for `part=1` play; off: back to 1 for 0,
//!   the menu's coin settings, only when it is free) at start; the engine has read them before it
//!   loads the library: when they change, the menus are started again.
//!
//! Natives: `start coin service test camera item boost accel brake left right`; the left stick
//! steers, the pedals throttle and brake.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use wal_protocol::Axis;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Coin,
    Service,
    Test,
    Camera,
    Item,
    Boost,
    Accel,
    Brake,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("service", Native::Service),
    ("test", Native::Test),
    ("camera", Native::Camera),
    ("item", Native::Item),
    ("boost", Native::Boost),
    ("accel", Native::Accel),
    ("brake", Native::Brake),
    ("left", Native::Left),
    ("right", Native::Right),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "boost"),
    ("b2", "item"),
    ("b3", "camera"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The natives held by player 1.
fn held() -> [bool; NATIVES.len()] {
    let mut held = [false; NATIVES.len()];
    map().for_each_pressed(&input(0), |n| held[n as usize] = true);
    held
}

/// The engine's fixed point (22.10) of `value / 32767`.
fn fixed(value: i32) -> i32 {
    value * 1024 / 32767
}

/// The wheel: -1..1, left positive.
#[unsafe(no_mangle)]
pub extern "C" fn WheelValue() -> i32 {
    let on = held();
    let lx = if on[Native::Left as usize] {
        -32767
    } else if on[Native::Right as usize] {
        32767
    } else {
        input(0).axis(Axis::LeftX) as i32
    };
    -fixed(lx)
}

/// The pedal of `native` (held: full) or `axis`: 0..1.
fn pedal(native: Native, axis: Axis) -> i32 {
    if held()[native as usize] { 1024 } else { fixed(input(0).axis(axis).max(0) as i32) }
}

#[unsafe(no_mangle)]
pub extern "C" fn AccelValue() -> i32 {
    pedal(Native::Accel, Axis::Accel)
}

#[unsafe(no_mangle)]
pub extern "C" fn BrakeValue() -> i32 {
    pedal(Native::Brake, Axis::Brake)
}

#[unsafe(no_mangle)]
pub extern "C" fn GetRealWheel() -> i32 {
    WheelValue() * 100
}

#[unsafe(no_mangle)]
pub extern "C" fn GetRealAccel() -> i32 {
    AccelValue() * 100
}

#[unsafe(no_mangle)]
pub extern "C" fn GetRealBrake() -> i32 {
    BrakeValue() * 100
}

/// The switches (an integer): bit 0 service, 1 test, 2 camera, 4 item, 5 boost, 6 start, 7 coin.
#[unsafe(no_mangle)]
pub extern "C" fn ButtonDown() -> i32 {
    let on = held();
    let bits = [
        (Native::Service, 0),
        (Native::Test, 1),
        (Native::Camera, 2),
        (Native::Item, 4),
        (Native::Boost, 5),
        (Native::Start, 6),
        (Native::Coin, 7),
    ]
    .iter()
    .filter(|(n, _)| on[*n as usize])
    .fold(0, |bits, (_, bit)| bits | 1 << bit);
    bits << 10
}

/// The board is on (the start lamp bit, as the community library).
#[unsafe(no_mangle)]
pub extern "C" fn inputIsOn() -> i32 {
    (1 << 6) << 10
}

#[unsafe(no_mangle)]
pub extern "C" fn inputInit(_a: i32, _b: i32) -> i32 {
    log!("senninha: inputInit");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn inputCenter() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn inputClose(_a: i32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn inputForce(_a: i32) -> i32 {
    1
}

/// As the community library: 1 when asked for a positive count.
#[unsafe(no_mangle)]
pub extern "C" fn inputGetCredit(count: i32) -> i32 {
    ((count > 0) as i32) << 10
}

#[unsafe(no_mangle)]
pub extern "C" fn TicketError() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn inputBreak() {}

macro_rules! ignored {
    ($($name:ident;)*) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "C" fn $name(value: i32) -> i32 {
                value >> 10
            }
        )*
    };
}

ignored! {
    inputLampadas;
    inputShake;
    inputSlamL;
    inputSlamR;
    inputTickets;
    inputVelocimetro;
}

/// The game's root: the folder above this library's (`bin\s_input.dll`).
fn root() -> Option<String> {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    let mut buf = [0u16; 512];
    let n = unsafe {
        GetModuleHandleExW(flags, root as *const () as *const u16, &mut module);
        GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32)
    } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    Some(path.rsplitn(3, '\\').nth(2)?.to_string())
}

/// `path` (a Windows path, `\` or `/`) from `dir` when relative, its `.` and `..` resolved.
fn resolve(dir: &str, path: &str) -> String {
    let path = path.replace('/', "\\");
    let absolute = path.starts_with('\\') || path.get(1..2) == Some(":");
    let full = if absolute { path } else { format!("{dir}\\{path}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in full.split('\\') {
        match part {
            "" | "." if !parts.is_empty() => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("\\")
}

/// The relaunch arguments of the program `file` started from `dir` with `args`: `exe=<file from
/// root> [cwd=<its folder>] <args>` (`/` paths); `None` outside the root.
fn relaunch_args(root: &str, dir: &str, file: &str, args: &str) -> Option<String> {
    let file = resolve(dir, file);
    let root = resolve("", root);
    // compared without case (Windows paths)
    let rel = file.get(root.len()..).filter(|_| file.get(..root.len()).is_some_and(|r| r.eq_ignore_ascii_case(&root)))?.strip_prefix('\\')?;
    // the launcher's paths (from its run directory)
    let rel = rel.replace('\\', "/");
    // the cabinet's helper back to the menus (it only starts `..\..\interface.exe`), and the
    // record holder's webcam photo (`fotos\j<n>_p<n>_m<n>.bmp`, then the menus; no webcam here:
    // the rankings keep their default picture): the menus
    if rel.eq_ignore_ascii_case("bin/winexec/winexec.exe") || rel.eq_ignore_ascii_case("bin/recordes/recordcam.exe") {
        return Some("exe=interface.exe".to_string());
    }
    let cwd = rel.rsplit_once('/').map(|(d, _)| format!(" cwd={d}")).unwrap_or_default();
    Some(format!("exe={rel}{cwd} {}", args.trim()).trim_end().to_string())
}

/// A command line's program and arguments (the first word, quoted or not).
fn split_command(cmdline: &str) -> (&str, &str) {
    let line = cmdline.trim();
    match line.strip_prefix('"') {
        Some(q) => q.split_once('"').unwrap_or((q, "")),
        None => line.split_once(' ').unwrap_or((line, "")),
    }
}

type CreateProcessA = unsafe extern "system" fn(*const c_char, *mut c_char, *const c_void, *const c_void, i32, u32, *const c_void, *const c_char, *const c_void, *mut c_void) -> i32;

static REAL_CREATE_PROCESS: OnceLock<CreateProcessA> = OnceLock::new();

/// The engine's `exec`: the next program, started by the launcher once this one ends.
unsafe extern "system" fn create_process_a(app: *const c_char, cmdline: *mut c_char, pa: *const c_void, ta: *const c_void, inherit: i32, flags: u32, env: *const c_void, cwd: *const c_char, si: *const c_void, pi: *mut c_void) -> i32 {
    let text = |p: *const c_char| (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
    let (app_s, cmd_s, cwd_s) = (text(app), text(cmdline), text(cwd));
    log!("senninha: CreateProcessA({app_s:?}, {cmd_s:?}, cwd {cwd_s:?})");
    let dir = cwd_s.or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned())).unwrap_or_default();
    let (file, args) = match (&app_s, &cmd_s) {
        (Some(a), Some(c)) => (a.as_str(), split_command(c).1),
        (Some(a), None) => (a.as_str(), ""),
        (None, Some(c)) => split_command(c),
        (None, None) => ("", ""),
    };
    if let Some(args) = root().and_then(|root| relaunch_args(&root, &dir, file, args)) {
        log!("senninha: next program by the launcher: {args}");
        wal_payload_common::request_relaunch(&args);
        // a process never signaled, for the engine's handles
        if !pi.is_null() {
            let event = || unsafe { windows_sys::Win32::System::Threading::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) } as usize;
            let pi = pi.cast::<usize>();
            unsafe {
                *pi = event();
                *pi.add(1) = event();
                *pi.add(2) = 0x5341;
                *pi.add(3) = 0x5342;
            }
        }
        return 1;
    }
    match REAL_CREATE_PROCESS.get() {
        Some(real) => unsafe { real(app, cmdline, pa, ta, inherit, flags, env, cwd, si, pi) },
        None => 0,
    }
}

/// `config_senninha.ini`'s `[Coin]` set to free play (`value=0`, `part=1`), or out of it (`value=1`,
/// `part=0`, only when `value` is 0); `None` when unchanged.
fn with_free_play(ini: &str, free: bool) -> Option<String> {
    let mut section = false;
    let mut free_now = false;
    let lines: Vec<&str> = ini.split_inclusive('\n').collect();
    for line in &lines {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.eq_ignore_ascii_case("[coin]");
        } else if section && t.replace(' ', "").eq_ignore_ascii_case("value=0") {
            free_now = true;
        }
    }
    if free == free_now {
        return None;
    }
    let (value, part) = if free { ("0", "1") } else { ("1", "0") };
    let mut section = false;
    let out = lines
        .iter()
        .map(|line| {
            let t = line.trim();
            if t.starts_with('[') {
                section = t.eq_ignore_ascii_case("[coin]");
            }
            let key = t.split_once('=').map(|(k, _)| k.trim().to_ascii_lowercase());
            let eol = &line[line.trim_end().len()..];
            match key.as_deref() {
                Some("value") if section => format!("value={value}{eol}"),
                Some("part") if section => format!("part={part}{eol}"),
                _ => line.to_string(),
            }
        })
        .collect();
    Some(out)
}

/// The common free play option in the game's settings; whether it changed them.
fn set_free_play(root: &str) -> bool {
    let Some(free) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") else { return false };
    let path = format!("{root}\\bin\\config_senninha.ini");
    match std::fs::read_to_string(&path).map(|ini| with_free_play(&ini, free)) {
        Ok(Some(ini)) => match std::fs::write(&path, ini) {
            Ok(()) => {
                log!("senninha: config_senninha.ini free play {free}");
                return true;
            }
            Err(e) => log!("senninha: config_senninha.ini not written: {e}"),
        },
        Ok(None) => {}
        Err(e) => log!("senninha: config_senninha.ini not read: {e}"),
    }
    false
}

/// The menus started again by the launcher, this program ended: the engine reads the settings
/// before it loads its plugins (this library).
fn restart_menus() {
    std::thread::spawn(|| {
        for _ in 0..100 {
            if wal_payload_common::connected() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        log!("senninha: settings changed: the menus started again");
        wal_payload_common::request_relaunch("exe=interface.exe");
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::process::exit(0);
    });
}

pub fn init() {
    if root().is_some_and(|root| set_free_play(&root)) {
        restart_menus();
        return;
    }
    let acknex = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"acknex.dll".as_ptr().cast()) } as usize;
    match unsafe { iat::hook_module(acknex, "kernel32.dll", "CreateProcessA", create_process_a as *const () as usize) } {
        Some(real) => {
            let _ = REAL_CREATE_PROCESS.set(unsafe { std::mem::transmute::<usize, CreateProcessA>(real) });
        }
        None => log!("senninha: acknex.dll does not import CreateProcessA"),
    }
    log!("senninha: s_input.dll, root {:?}", root());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch() {
        let root = "C:\\wal\\game";
        assert_eq!(relaunch_args(root, root, "bin\\asia\\seninha.exe", "-cr3").as_deref(), Some("exe=bin/asia/seninha.exe cwd=bin/asia -cr3"));
        assert_eq!(relaunch_args(root, "C:\\WAL\\Game\\bin\\asia", "..\\..\\interface.exe", "").as_deref(), Some("exe=interface.exe"));
        assert_eq!(relaunch_args(root, "C:\\wal\\game\\bin", "C:/wal/game/bin/podium/podium.exe", " a b ").as_deref(), Some("exe=bin/podium/podium.exe cwd=bin/podium a b"));
        assert_eq!(relaunch_args(root, root, "C:\\windows\\notepad.exe", ""), None);
        assert_eq!(relaunch_args(root, "C:\\wal\\game\\bin\\asia", "..\\winexec\\winexec.exe", "").as_deref(), Some("exe=interface.exe"));
        assert_eq!(relaunch_args(root, "C:\\wal\\game\\bin\\asia", "..\\recordes\\recordcam.exe", "3 1").as_deref(), Some("exe=interface.exe"));
        assert_eq!(split_command("\"a b.exe\" -x"), ("a b.exe", " -x"));
    }

    #[test]
    fn free_play() {
        let ini = "[GameConfig]\r\nvalue=1\r\n\r\n[Coin]\r\nvalue=1\r\npart=0\r\n";
        let free = with_free_play(ini, true).unwrap();
        assert_eq!(free, "[GameConfig]\r\nvalue=1\r\n\r\n[Coin]\r\nvalue=0\r\npart=1\r\n");
        assert_eq!(with_free_play(&free, true), None);
        assert_eq!(with_free_play(&free, false).as_deref(), Some(ini));
        assert_eq!(with_free_play("[Coin]\nvalue=2\npart=1", false), None);
    }
}
