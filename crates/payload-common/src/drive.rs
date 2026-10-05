//! Redirects the game's `D:\` accesses (arcade cabinets keep their data on drive D:) to a
//! folder of the game directory, so the Wine prefix needs no D: drive.
//!
//! The folder is given by a system-specific environment variable (e.g. `WAL_NESICA_DDRIVE`),
//! relative to the game directory or absolute (Windows path), with a system default
//! (`WindowsLoader`, the WindowsLoader layout, so existing saves keep working).
//!
//! Done with IAT hooks on the game executable, like WindowsLoader's path hooks, and on the
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
static CONFIG: OnceLock<(String, String)> = OnceLock::new();

fn target() -> &'static Target {
    TARGET.get_or_init(|| {
        let (var, default) = CONFIG.get().cloned().unwrap_or_else(|| ("WAL_DDRIVE".into(), "WindowsLoader".into()));
        let folder = std::env::var(&var).unwrap_or(default);
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
            // keep the directory with its trailing separator
            ansi.truncate(ansi.iter().rposition(|c| *c == b'\\').map_or(0, |i| i + 1));
            wide.truncate(wide.iter().rposition(|c| *c == b'\\' as u16).map_or(0, |i| i + 1));
        }
        ansi.extend_from_slice(folder.as_bytes());
        wide.extend(folder.encode_utf16());
        log!("drive: D:\\ redirected to {}", String::from_utf16_lossy(&wide));
        Target { ansi, wide }
    })
}

/// Creates the D: data folder.
fn prepare_data_dir() {
    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log!("drive: cannot create {dir}: {e}");
    }
}

/// Windows path of the folder holding the game's D: data.
pub fn data_dir() -> String {
    String::from_utf16_lossy(&target().wide)
}

fn ansi(p: *const u8) -> Redirected<u8> {
    unsafe { rewrite_drive(p, b'D', &target().ansi) }
}

fn wide(p: *const u16) -> Redirected<u16> {
    unsafe { rewrite_drive(p, b'D', &target().wide) }
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

        /// Installs the redirection; `var` names the folder variable, `default` its default.
        pub fn init(var: &str, default: &str) {
            let _ = CONFIG.set((var.to_string(), default.to_string()));
            prepare_data_dir();
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
    16 SetCurrentDirectoryA [ansi; path: *const u8] () -> i32;
    17 SetCurrentDirectoryW [wide; path: *const u16] () -> i32;
    18 GetDiskFreeSpaceExA [ansi; path: *const u8] (avail: P, total: P, free: P) -> i32;
    19 GetDiskFreeSpaceExW [wide; path: *const u16] (avail: P, total: P, free: P) -> i32;
    20 MoveFileA [ansi; from: *const u8, to: *const u8] () -> i32;
    21 MoveFileW [wide; from: *const u16, to: *const u16] () -> i32;
    22 MoveFileExA [ansi; from: *const u8, to: *const u8] (flags: u32) -> i32;
    23 MoveFileExW [wide; from: *const u16, to: *const u16] (flags: u32) -> i32;
    24 CopyFileA [ansi; from: *const u8, to: *const u8] (fail_if_exists: i32) -> i32;
    25 CopyFileW [wide; from: *const u16, to: *const u16] (fail_if_exists: i32) -> i32;
}
