//! Redirects the game's `D:\` accesses (NESiCA cabinets keep their data on drive D:)
//! to a folder of the game directory, so the Wine prefix needs no D: drive.
//!
//! Default target: `WindowsLoader` next to the game executable (same place as WindowsLoader,
//! existing saves keep working). `WAL_NESICA_DDRIVE` overrides it, relative to the game
//! directory or absolute (Windows path).
//!
//! Done with IAT hooks on the game executable, like WindowsLoader's RfidEmu path hooks.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::paths::{Redirected, rewrite_drive};
use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameA, GetModuleFileNameW};

struct Target {
    ansi: Vec<u8>,
    wide: Vec<u16>,
}

static TARGET: OnceLock<Target> = OnceLock::new();

fn target() -> &'static Target {
    TARGET.get_or_init(|| {
        let folder = std::env::var("WAL_NESICA_DDRIVE").unwrap_or_else(|_| "WindowsLoader".into());
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

/// Creates the D: data folder and the NESYS news picture WindowsLoader provides there.
fn prepare_data_dir() {
    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log!("drive: cannot create {dir}: {e}");
    }
    let news = format!("{dir}\\news.png");
    if !std::path::Path::new(&news).exists() {
        let _ = std::fs::write(&news, include_bytes!("news.png"));
    }
}

/// Windows path of the folder holding the game's D: data.
pub(crate) fn data_dir() -> String {
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

        pub(crate) fn init() {
            prepare_data_dir();
            $(
                if let Some(o) = unsafe { iat::hook("kernel32.dll", stringify!($name), $name as *const () as usize) } {
                    ORIG[$idx].store(o, Ordering::Relaxed);
                }
            )*
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
