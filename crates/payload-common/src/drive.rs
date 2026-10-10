//! Redirects the game's `D:\` accesses (arcade cabinets keep their data on drive D:) to a
//! folder of the game directory, so the Wine prefix needs no D: drive. Other letters work the
//! same ([`init_letter`]: Global VR games run from a `subst W: .` drive).
//!
//! The folder is given by a system-specific environment variable (e.g. `WAL_NESICA_DDRIVE`),
//! relative to the game directory (`.`: the game directory itself) or absolute (Windows path),
//! with a system default (`WindowsLoader`).
//!
//! Done with IAT hooks on the game executable and on the
//! C runtime DLLs it has loaded: games doing `fopen("D:/...")` go through the CRT's own
//! kernel32 imports (Chaos Code crashed on `fseek` of a NULL `FILE` otherwise).


#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::paths::{Redirected, rewrite_drive};
use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameA, GetModuleFileNameW, GetModuleHandleA};

/// C runtimes whose file functions are redirected too, when loaded with the game.
const CRT_MODULES: [&str; 9] = [
    "msvcrt.dll", "msvcr70.dll", "msvcr71.dll", "msvcr80.dll", "msvcr90.dll", "msvcr100.dll",
    "msvcr110.dll", "msvcr120.dll", "ucrtbase.dll",
];

struct Target {
    ansi: Vec<u8>,
    wide: Vec<u16>,
}

static TARGET: OnceLock<Target> = OnceLock::new();
struct Config {
    letter: u8,
    var: String,
    default: String,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

fn letter() -> u8 {
    CONFIG.get().map_or(b'D', |c| c.letter)
}

fn target() -> &'static Target {
    TARGET.get_or_init(|| {
        let (var, default) = CONFIG.get().map_or(("WAL_DDRIVE", "WindowsLoader"), |c| (&c.var, &c.default));
        let folder = std::env::var(var).unwrap_or_else(|_| default.to_string());
        let absolute = folder.contains(':') || folder.starts_with('\\');
        let mut ansi = vec![0u8; 1024];
        let mut wide = vec![0u16; 1024];
        unsafe {
            let n = GetModuleFileNameA(std::ptr::null_mut(), ansi.as_mut_ptr(), ansi.len() as u32) as usize;
            ansi.truncate(n);
            let n = GetModuleFileNameW(std::ptr::null_mut(), wide.as_mut_ptr(), wide.len() as u32) as usize;
            wide.truncate(n);
        }
        if absolute {
            ansi.clear();
            wide.clear();
        } else {
            // the directory, with its trailing separator unless it is the target itself
            let keep = if folder == "." { 0 } else { 1 };
            ansi.truncate(ansi.iter().rposition(|c| *c == b'\\').map_or(0, |i| i + keep));
            wide.truncate(wide.iter().rposition(|c| *c == b'\\' as u16).map_or(0, |i| i + keep));
        }
        if folder != "." {
            ansi.extend_from_slice(folder.as_bytes());
            wide.extend(folder.encode_utf16());
        }
        log!("drive: {}:\\ redirected to {}", letter() as char, String::from_utf16_lossy(&wide));
        Target { ansi, wide }
    })
}

/// Creates the data folder.
fn prepare_data_dir() {
    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log!("drive: cannot create {dir}: {e}");
    }
}

/// Windows path of the folder holding the redirected drive's data.
pub fn data_dir() -> String {
    String::from_utf16_lossy(&target().wide)
}

fn ansi(p: *const u8) -> Redirected<u8> {
    let r = unsafe { rewrite_drive(p, letter(), &target().ansi) };
    match game_drive() {
        Some(g) if !r.is_replaced() => unsafe { rewrite_drive(p, g.letter, &g.ansi) },
        _ => r,
    }
}

fn wide(p: *const u16) -> Redirected<u16> {
    let r = unsafe { rewrite_drive(p, letter(), &target().wide) };
    match game_drive() {
        Some(g) if !r.is_replaced() => unsafe { rewrite_drive(p, g.letter, &g.wide) },
        _ => r,
    }
}

/// `WAL_GAME_DRIVE=<letter>`: the game runs from the root of that drive, as on the cabinet
/// (Battle Gear 4: `E:\`, its data found from the current directory's drive). Paths on that
/// drive go to the game directory, `GetCurrentDirectory` answers its root, and
/// `GetLogicalDrives` / `GetDriveType` report C:, the redirected drive and it as fixed disks.
struct GameDrive {
    letter: u8,
    ansi: Vec<u8>,
    wide: Vec<u16>,
}

fn game_drive() -> Option<&'static GameDrive> {
    static DRIVE: OnceLock<Option<GameDrive>> = OnceLock::new();
    DRIVE
        .get_or_init(|| {
            let letter = std::env::var("WAL_GAME_DRIVE").ok()?.bytes().next()?.to_ascii_uppercase();
            if !letter.is_ascii_uppercase() {
                return None;
            }
            // the game directory, without its trailing separator
            let mut ansi = vec![0u8; 1024];
            let mut wide = vec![0u16; 1024];
            unsafe {
                let n = GetModuleFileNameA(std::ptr::null_mut(), ansi.as_mut_ptr(), ansi.len() as u32) as usize;
                ansi.truncate(n);
                let n = GetModuleFileNameW(std::ptr::null_mut(), wide.as_mut_ptr(), wide.len() as u32) as usize;
                wide.truncate(n);
            }
            ansi.truncate(ansi.iter().rposition(|c| *c == b'\\').unwrap_or(0));
            wide.truncate(wide.iter().rposition(|c| *c == b'\\' as u16).unwrap_or(0));
            log!("drive: {}:\\ redirected to {} (the game's drive)", letter as char, String::from_utf16_lossy(&wide));
            Some(GameDrive { letter, ansi, wide })
        })
        .as_ref()
}

fn drive_bits() -> u32 {
    let mut bits = 1 << 2 | 1 << (letter() - b'A');
    if let Some(g) = game_drive() {
        bits |= 1 << (g.letter - b'A');
    }
    bits
}

static GAME_DRIVE_ORIG: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

unsafe extern "system" fn GetLogicalDrives() -> u32 {
    let original: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(GAME_DRIVE_ORIG[0].load(Ordering::Relaxed)) };
    drive_bits() | unsafe { original() }
}

/// DRIVE_FIXED for the reported drives (Wine answers DRIVE_NO_ROOT_DIR for the absent ones).
fn drive_type(first: u32, original: impl FnOnce() -> u32) -> u32 {
    let c = (first as u8).to_ascii_uppercase();
    if c.is_ascii_uppercase() && drive_bits() & (1 << (c - b'A')) != 0 { 3 } else { original() }
}

unsafe extern "system" fn GetDriveTypeA(root: *const u8) -> u32 {
    let original: unsafe extern "system" fn(*const u8) -> u32 = unsafe { std::mem::transmute(GAME_DRIVE_ORIG[1].load(Ordering::Relaxed)) };
    let first = if root.is_null() { 0 } else { u32::from(unsafe { *root }) };
    drive_type(first, || unsafe { original(root) })
}

unsafe extern "system" fn GetDriveTypeW(root: *const u16) -> u32 {
    let original: unsafe extern "system" fn(*const u16) -> u32 = unsafe { std::mem::transmute(GAME_DRIVE_ORIG[2].load(Ordering::Relaxed)) };
    let first = if root.is_null() { 0 } else { u32::from(unsafe { *root }) };
    drive_type(first, || unsafe { original(root) })
}

/// The game drive's root, `X:\` (3 characters): the length without the NUL when it fits, the
/// size needed (with the NUL) otherwise.
unsafe fn current_dir<T: From<u8>>(size: u32, buffer: *mut T) -> u32 {
    let letter = game_drive().map_or(b'C', |g| g.letter);
    if size < 4 || buffer.is_null() {
        return 4;
    }
    for (i, c) in [letter, b':', b'\\', 0].into_iter().enumerate() {
        unsafe { buffer.add(i).write(T::from(c)) };
    }
    3
}

unsafe extern "system" fn GetCurrentDirectoryA(size: u32, buffer: *mut u8) -> u32 {
    unsafe { current_dir(size, buffer) }
}

unsafe extern "system" fn GetCurrentDirectoryW(size: u32, buffer: *mut u16) -> u32 {
    unsafe { current_dir(size, buffer) }
}

static MMIO_ORIG: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

/// `winmm!mmioOpenA/W`: RIFF files opened by path (Battle Gear 4's music, `E:\data\Sound\Bgm`).
unsafe extern "system" fn mmioOpenA(path: *mut u8, info: P, flags: u32) -> P {
    let original: unsafe extern "system" fn(*mut u8, P, u32) -> P = unsafe { std::mem::transmute(MMIO_ORIG[0].load(Ordering::Relaxed)) };
    let p = ansi(path);
    unsafe { original(p.ptr().cast_mut(), info, flags) }
}

unsafe extern "system" fn mmioOpenW(path: *mut u16, info: P, flags: u32) -> P {
    let original: unsafe extern "system" fn(*mut u16, P, u32) -> P = unsafe { std::mem::transmute(MMIO_ORIG[1].load(Ordering::Relaxed)) };
    let p = wide(path);
    unsafe { original(p.ptr().cast_mut(), info, flags) }
}

fn init_mmio() {
    for (i, (name, f)) in [("mmioOpenA", mmioOpenA as *const () as usize), ("mmioOpenW", mmioOpenW as *const () as usize)].into_iter().enumerate() {
        if let Some(o) = unsafe { iat::hook("winmm.dll", name, f) } {
            MMIO_ORIG[i].store(o, Ordering::Relaxed);
        }
    }
}

/// Installs the game drive hooks (game executable only) when `WAL_GAME_DRIVE` is set.
fn init_game_drive() {
    if game_drive().is_none() {
        return;
    }
    let hooks: [(&str, usize); 5] = [
        ("GetLogicalDrives", GetLogicalDrives as *const () as usize),
        ("GetDriveTypeA", GetDriveTypeA as *const () as usize),
        ("GetDriveTypeW", GetDriveTypeW as *const () as usize),
        ("GetCurrentDirectoryA", GetCurrentDirectoryA as *const () as usize),
        ("GetCurrentDirectoryW", GetCurrentDirectoryW as *const () as usize),
    ];
    for (i, (name, f)) in hooks.into_iter().enumerate() {
        if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            GAME_DRIVE_ORIG[i].store(o, Ordering::Relaxed);
        }
    }
}

/// `WAL_PIN_CWD=1`: the game's working directory stays its own directory (Type X2
/// `SetCurrentDirectoryA`): `.\sh` and `.\data\sh` go to those folders of the
/// game directory, anything else to the game directory (Gaia Attack 4 steps up with `..\`
/// and no longer finds `data\sound`).
fn pin_cwd() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_PIN_CWD").is_ok_and(|v| v == "1"))
}

/// The pinned directory for a `SetCurrentDirectory` argument.
fn pinned_dir(requested: &str) -> String {
    let mut exe = vec![0u16; 1024];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), exe.as_mut_ptr(), exe.len() as u32) } as usize;
    let exe = String::from_utf16_lossy(&exe[..n]);
    let dir = exe.rsplit_once('\\').map_or(exe.as_str(), |(d, _)| d).to_string();
    let lower = requested.to_ascii_lowercase().replace('/', "\\");
    let dir = if lower.starts_with(".\\sh") {
        format!("{dir}\\sh")
    } else if lower.starts_with(".\\data\\sh") {
        format!("{dir}\\data\\sh")
    } else {
        dir
    };
    log!("drive: SetCurrentDirectory {requested:?} -> {dir}");
    dir
}

fn cwd_ansi(p: *const u8) -> Redirected<u8> {
    if !pin_cwd() || p.is_null() {
        return ansi(p);
    }
    let requested = unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned();
    let mut out = pinned_dir(&requested).into_bytes();
    out.push(0);
    Redirected::replaced(out)
}

fn cwd_wide(p: *const u16) -> Redirected<u16> {
    if !pin_cwd() || p.is_null() {
        return wide(p);
    }
    let mut len = 0;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    let requested = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) });
    let mut out: Vec<u16> = pinned_dir(&requested).encode_utf16().collect();
    out.push(0);
    Redirected::replaced(out)
}

const COUNT: usize = 26;
static ORIG: [AtomicUsize; COUNT] = [const { AtomicUsize::new(0) }; COUNT];

macro_rules! hooks {
    ($($idx:literal $name:ident [$conv:ident; $($p:ident: $pt:ty),+] ($($a:ident: $t:ty),*) -> $ret:ty;)*) => {
        $(
            unsafe extern "system" fn $name($($p: $pt,)+ $($a: $t),*) -> $ret {
                $(let $p = $conv($p);)+
                let original: unsafe extern "system" fn($($pt,)+ $($t),*) -> $ret =
                    unsafe { std::mem::transmute(ORIG[$idx].load(Ordering::Relaxed)) };
                unsafe { original($($p.ptr(),)+ $($a),*) }
            }
        )*

        /// Installs the `D:` redirection; `var` names the folder variable, `default` its default.
        pub fn init(var: &str, default: &str) {
            init_letter(b'D', var, default)
        }

        /// Installs the redirection of drive `letter`.
        pub fn init_letter(letter: u8, var: &str, default: &str) {
            let _ = CONFIG.set(Config { letter: letter.to_ascii_uppercase(), var: var.to_string(), default: default.to_string() });
            prepare_data_dir();
            init_game_drive();
            init_mmio();
            $(
                if let Some(o) = unsafe { iat::hook("kernel32.dll", stringify!($name), $name as *const () as usize) } {
                    ORIG[$idx].store(o, Ordering::Relaxed);
                }
            )*
            for crt in CRT_MODULES {
                let name = format!("{crt}\0");
                let base = unsafe { GetModuleHandleA(name.as_ptr()) } as usize;
                if base == 0 {
                    continue;
                }
                log!("drive: redirecting {crt} file functions");
                $(
                    if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", stringify!($name), $name as *const () as usize) } {
                        // the CRT and the game import the same kernel32 function
                        let _ = ORIG[$idx].compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
                    }
                )*
            }
        }
    };
}

type P = *mut c_void;

hooks! {
    0 CreateFileA [ansi; path: *const u8] (access: u32, share: u32, sa: P, disposition: u32, flags: u32, template: P) -> P;
    1 CreateFileW [wide; path: *const u16] (access: u32, share: u32, sa: P, disposition: u32, flags: u32, template: P) -> P;
    2 GetFileAttributesA [ansi; path: *const u8] () -> u32;
    3 GetFileAttributesW [wide; path: *const u16] () -> u32;
    4 GetFileAttributesExA [ansi; path: *const u8] (level: i32, info: P) -> i32;
    5 GetFileAttributesExW [wide; path: *const u16] (level: i32, info: P) -> i32;
    6 CreateDirectoryA [ansi; path: *const u8] (sa: P) -> i32;
    7 CreateDirectoryW [wide; path: *const u16] (sa: P) -> i32;
    8 RemoveDirectoryA [ansi; path: *const u8] () -> i32;
    9 RemoveDirectoryW [wide; path: *const u16] () -> i32;
    10 DeleteFileA [ansi; path: *const u8] () -> i32;
    11 DeleteFileW [wide; path: *const u16] () -> i32;
    12 FindFirstFileA [ansi; path: *const u8] (data: P) -> P;
    13 FindFirstFileW [wide; path: *const u16] (data: P) -> P;
    14 FindFirstFileExA [ansi; path: *const u8] (level: i32, data: P, op: i32, filter: P, flags: u32) -> P;
    15 FindFirstFileExW [wide; path: *const u16] (level: i32, data: P, op: i32, filter: P, flags: u32) -> P;
    16 SetCurrentDirectoryA [cwd_ansi; path: *const u8] () -> i32;
    17 SetCurrentDirectoryW [cwd_wide; path: *const u16] () -> i32;
    18 GetDiskFreeSpaceExA [ansi; path: *const u8] (avail: P, total: P, free: P) -> i32;
    19 GetDiskFreeSpaceExW [wide; path: *const u16] (avail: P, total: P, free: P) -> i32;
    20 MoveFileA [ansi; from: *const u8, to: *const u8] () -> i32;
    21 MoveFileW [wide; from: *const u16, to: *const u16] () -> i32;
    22 MoveFileExA [ansi; from: *const u8, to: *const u8] (flags: u32) -> i32;
    23 MoveFileExW [wide; from: *const u16, to: *const u16] (flags: u32) -> i32;
    24 CopyFileA [ansi; from: *const u8, to: *const u8] (fail_if_exists: i32) -> i32;
    25 CopyFileW [wide; from: *const u16, to: *const u16] (fail_if_exists: i32) -> i32;
}
