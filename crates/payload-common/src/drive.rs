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
//! `WAL_DRIVE_LATE_DLLS=1`: the DLLs of the game's directory it loads later too
//! (`late_hooks`): Half-Life 2: Survivor's `bin\launcher.dll` reads `D:\config.bin` with its
//! own C runtime.


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

/// The redirected drive of [`init_letter`] (none with [`init_drives`] alone).
fn letter() -> Option<u8> {
    CONFIG.get().map(|c| c.letter)
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
        log!("drive: {}:\\ redirected to {}", letter().unwrap_or(b'D') as char, String::from_utf16_lossy(&wide));
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

/// A folder of the drive's root that is the target folder itself (`D:\\jd6\\...` on a cabinet
/// whose game is `D:\\jd6`): [`skip_folder`].
static SKIP: OnceLock<String> = OnceLock::new();

/// Paths in `folder` of the redirected drive's root land in the target folder itself, the
/// folder name dropped (the dump is that folder's content).
pub fn skip_folder(folder: &str) {
    let _ = SKIP.set(folder.to_ascii_lowercase());
}

/// `path` (a redirected one, `target` its first `target_len` elements) without the skipped
/// folder after the target.
fn skip<T: Copy + Into<u32>>(mut r: Redirected<T>, target_len: usize) -> Redirected<T> {
    let Some(folder) = SKIP.get() else { return r };
    if let Some(v) = r.replaced.as_mut() {
        let n = folder.len();
        let rest = v.get(target_len + 1..target_len + 1 + n);
        let sep = v.get(target_len + 1 + n).map(|c| (*c).into());
        let matches = rest.is_some_and(|r| r.iter().zip(folder.bytes()).all(|(c, f)| {
            let c: u32 = (*c).into();
            c < 128 && (c as u8).to_ascii_lowercase() == f
        }));
        if matches && matches!(sep, Some(0x5c | 0x2f)) {
            v.drain(target_len + 1..target_len + 2 + n);
        }
    }
    r
}

fn ansi(p: *const u8) -> Redirected<u8> {
    if let Some(letter) = letter() {
        let r = skip(unsafe { rewrite_drive(p, letter, &target().ansi) }, target().ansi.len());
        if r.is_replaced() {
            return r;
        }
    }
    if let Some(g) = game_drive() {
        let r = unsafe { rewrite_drive(p, g.letter, &g.ansi) };
        if r.is_replaced() {
            return r;
        }
    }
    for d in mapped() {
        let r = unsafe { rewrite_drive(p, d.letter, &d.ansi) };
        if r.is_replaced() {
            return r;
        }
    }
    Redirected { original: p, replaced: None }
}

fn wide(p: *const u16) -> Redirected<u16> {
    if let Some(letter) = letter() {
        let r = skip(unsafe { rewrite_drive(p, letter, &target().wide) }, target().wide.len());
        if r.is_replaced() {
            return r;
        }
    }
    if let Some(g) = game_drive() {
        let r = unsafe { rewrite_drive(p, g.letter, &g.wide) };
        if r.is_replaced() {
            return r;
        }
    }
    for d in mapped() {
        let r = unsafe { rewrite_drive(p, d.letter, &d.wide) };
        if r.is_replaced() {
            return r;
        }
    }
    Redirected { original: p, replaced: None }
}

/// A drive of the launcher's `WAL_DRIVES` (the profile's cabinet drives): its folder.
struct MappedDrive {
    letter: u8,
    ansi: Vec<u8>,
    wide: Vec<u16>,
}

/// `WAL_DRIVES=<letter>=<Windows path>|...`: the profile's cabinet drives (E:, Y:...),
/// redirected to their folders whatever Wine's drive letters are (its mount manager gives a
/// letter to each disk it finds and replaces the launcher's link of that letter: a USB disk
/// took E:, Initial D ARCADE STAGE Zero's AMFS drive, amdaemon error 4105).
fn mapped() -> &'static [MappedDrive] {
    static DRIVES: OnceLock<Vec<MappedDrive>> = OnceLock::new();
    DRIVES.get_or_init(|| {
        let Ok(spec) = std::env::var(wal_protocol::env::DRIVES) else { return Vec::new() };
        spec.split('|')
            .filter_map(|entry| {
                let (letter, path) = entry.split_once('=')?;
                let letter = letter.trim().bytes().next()?.to_ascii_uppercase();
                if !letter.is_ascii_uppercase() || path.is_empty() {
                    return None;
                }
                let path = path.trim_end_matches('\\');
                log!("drive: {}:\\ redirected to {path}", letter as char);
                Some(MappedDrive { letter, ansi: ansi_path(path), wide: path.encode_utf16().collect() })
            })
            .collect()
    })
}

/// `path` in the ANSI code page (the narrow file functions' paths).
fn ansi_path(path: &str) -> Vec<u8> {
    use windows_sys::Win32::Globalization::{CP_ACP, WideCharToMultiByte};
    if path.is_ascii() {
        return path.as_bytes().to_vec();
    }
    let wide: Vec<u16> = path.encode_utf16().collect();
    let mut out = vec![0u8; wide.len() * 4];
    let n = unsafe {
        WideCharToMultiByte(CP_ACP, 0, wide.as_ptr(), wide.len() as i32, out.as_mut_ptr(), out.len() as i32, std::ptr::null(), std::ptr::null_mut())
    };
    out.truncate(n.max(0) as usize);
    out
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
    let mut bits = 1 << 2;
    if let Some(letter) = letter() {
        bits |= 1 << (letter - b'A');
    }
    if let Some(g) = game_drive() {
        bits |= 1 << (g.letter - b'A');
    }
    for d in mapped() {
        bits |= 1 << (d.letter - b'A');
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

/// SQLite's open functions (`sqlite3.dll`, a Unity game's `Mono.Data.Sqlite`): its file calls
/// go through kernel32 pointers it copied at its load, before any import hook, so a database on
/// a redirected drive (`D:/SettingData.db`) would open on Wine's own letter (whatever its mount
/// manager gave it: another disk). The path rewritten here as the file functions' are.
static SQLITE_ORIG: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// A UTF-8 path rewritten (through its UTF-16 form), `None` when not on a redirected drive.
fn utf8_redirected(p: *const u8) -> Option<Vec<u8>> {
    if p.is_null() {
        return None;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_str().ok()?;
    let wide_path: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let r = wide(wide_path.as_ptr());
    let new = r.replaced.as_ref()?;
    let new = String::from_utf16_lossy(&new[..new.len() - 1]);
    log!("drive: sqlite {text} -> {new}");
    Some(new.into_bytes().into_iter().chain([0]).collect())
}

unsafe extern "C" fn sqlite3_open(path: *const u8, db: P) -> i32 {
    let original: unsafe extern "C" fn(*const u8, P) -> i32 = unsafe { std::mem::transmute(SQLITE_ORIG[0].load(Ordering::Relaxed)) };
    let new = utf8_redirected(path);
    unsafe { original(new.as_ref().map_or(path, |v| v.as_ptr()), db) }
}

unsafe extern "C" fn sqlite3_open_v2(path: *const u8, db: P, flags: i32, vfs: *const u8) -> i32 {
    let original: unsafe extern "C" fn(*const u8, P, i32, *const u8) -> i32 = unsafe { std::mem::transmute(SQLITE_ORIG[1].load(Ordering::Relaxed)) };
    let new = utf8_redirected(path);
    unsafe { original(new.as_ref().map_or(path, |v| v.as_ptr()), db, flags, vfs) }
}

unsafe extern "C" fn sqlite3_open16(path: *const u16, db: P) -> i32 {
    let original: unsafe extern "C" fn(*const u16, P) -> i32 = unsafe { std::mem::transmute(SQLITE_ORIG[2].load(Ordering::Relaxed)) };
    let p = wide(path);
    unsafe { original(p.ptr(), db) }
}

/// The payload's stand-in for SQLite's open function `name` (`real` its own), for the
/// `DllImport`s resolved by `mono_imports`; `None` for another function or without a redirect.
pub fn sqlite_import(name: &str, real: usize) -> Option<usize> {
    if real == 0 || (letter().is_none() && game_drive().is_none() && mapped().is_empty()) {
        return None;
    }
    let (i, f) = match name {
        "sqlite3_open" => (0, sqlite3_open as *const () as usize),
        "sqlite3_open_v2" => (1, sqlite3_open_v2 as *const () as usize),
        "sqlite3_open16" => (2, sqlite3_open16 as *const () as usize),
        _ => return None,
    };
    SQLITE_ORIG[i].store(real, Ordering::Relaxed);
    Some(f)
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

/// The game's `mmioOpen` imports, or those of the module at `base` (a game DLL).
fn init_mmio(base: Option<usize>) {
    for (i, (name, f)) in [("mmioOpenA", mmioOpenA as *const () as usize), ("mmioOpenW", mmioOpenW as *const () as usize)].into_iter().enumerate() {
        let hooked = match base {
            None => unsafe { iat::hook("winmm.dll", name, f) },
            Some(base) => unsafe { iat::hook_module(base, "winmm.dll", name, f) },
        };
        if let Some(o) = hooked {
            let _ = MMIO_ORIG[i].compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
        }
    }
}

static GRAPH_ORIG: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

type Guid = windows_sys::core::GUID;
const CLSID_FILTER_GRAPH: Guid = Guid::from_u128(0xe436ebb3_524f_11ce_9f53_0020af0ba770);
const IID_IGRAPH_BUILDER: Guid = Guid::from_u128(0x56a868a9_0ad4_11ce_b03a_0020af0ba770);
const IID_IFILTER_GRAPH2: Guid = Guid::from_u128(0x36b73882_c2c8_11cf_8b46_00805f6cef60);

fn same(a: &Guid, b: &Guid) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

/// `IGraphBuilder::RenderFile(file, playlist)` (method 13).
unsafe extern "system" fn RenderFile(this: P, file: *const u16, playlist: *const u16) -> i32 {
    let original: unsafe extern "system" fn(P, *const u16, *const u16) -> i32 = unsafe { std::mem::transmute(GRAPH_ORIG[1].load(Ordering::Relaxed)) };
    let f = wide(file);
    unsafe { original(this, f.ptr(), playlist) }
}

/// `IGraphBuilder::AddSourceFilter(file, name, filter)` (method 14).
unsafe extern "system" fn AddSourceFilter(this: P, file: *const u16, name: *const u16, filter: *mut P) -> i32 {
    let original: unsafe extern "system" fn(P, *const u16, *const u16, *mut P) -> i32 = unsafe { std::mem::transmute(GRAPH_ORIG[2].load(Ordering::Relaxed)) };
    let f = wide(file);
    unsafe { original(this, f.ptr(), name, filter) }
}

/// `ole32!CoCreateInstance`: a DirectShow filter graph opens the game's movies itself (Battle
/// Gear 4's attract movie, `E:/data/Scene/demo/attract/attract_demo.wmv`: not found, then a crash
/// on its never created player; Winning Eleven 2008's on D:): its `RenderFile` / `AddSourceFilter`
/// get the redirected drive's and the game drive's paths redirected too (the graph's vtable,
/// shared by its instances, patched once).
unsafe extern "system" fn CoCreateInstance(clsid: *const Guid, outer: P, ctx: u32, iid: *const Guid, out: *mut P) -> i32 {
    let original: unsafe extern "system" fn(*const Guid, P, u32, *const Guid, *mut P) -> i32 = unsafe { std::mem::transmute(GRAPH_ORIG[0].load(Ordering::Relaxed)) };
    let hr = unsafe { original(clsid, outer, ctx, iid, out) };
    let graph = !clsid.is_null() && !iid.is_null() && same(unsafe { &*clsid }, &CLSID_FILTER_GRAPH)
        && (same(unsafe { &*iid }, &IID_IGRAPH_BUILDER) || same(unsafe { &*iid }, &IID_IFILTER_GRAPH2));
    if hr >= 0 && graph && !out.is_null() && GRAPH_ORIG[2].load(Ordering::Relaxed) == 0 {
        unsafe { patch_graph(*out) };
    }
    hr
}

unsafe fn patch_graph(object: P) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    if object.is_null() {
        return;
    }
    let vtable = unsafe { *(object as *const *mut usize) };
    for (slot, index, f) in [(13usize, 1usize, RenderFile as *const () as usize), (14, 2, AddSourceFilter as *const () as usize)] {
        let entry = unsafe { vtable.add(slot) };
        let mut old = 0;
        unsafe {
            if VirtualProtect(entry.cast(), size_of::<usize>(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                continue;
            }
            GRAPH_ORIG[index].store(*entry, Ordering::Relaxed);
            *entry = f;
            VirtualProtect(entry.cast(), size_of::<usize>(), old, &mut old);
        }
    }
    log!("drive: DirectShow filter graph: its file paths on the game's drive redirected");
}

/// Installs the DirectShow hook, and the game drive hooks (game executable only) when
/// `WAL_GAME_DRIVE` is set.
fn init_game_drive() {
    // Winning Eleven 2008's attract movies (`D:/HCK/data/movie`): the redirected drive too
    if let Some(o) = unsafe { iat::hook("ole32.dll", "CoCreateInstance", CoCreateInstance as *const () as usize) } {
        GRAPH_ORIG[0].store(o, Ordering::Relaxed);
    }
    if game_drive().is_none() && mapped().is_empty() {
        return;
    }
    // the current directory is the game drive's root: with the cabinet drives alone, it is
    // left to Wine
    let count = if game_drive().is_some() { 5 } else { 3 };
    let hooks: [(&str, usize); 5] = [
        ("GetLogicalDrives", GetLogicalDrives as *const () as usize),
        ("GetDriveTypeA", GetDriveTypeA as *const () as usize),
        ("GetDriveTypeW", GetDriveTypeW as *const () as usize),
        ("GetCurrentDirectoryA", GetCurrentDirectoryA as *const () as usize),
        ("GetCurrentDirectoryW", GetCurrentDirectoryW as *const () as usize),
    ];
    for (i, (name, f)) in hooks.into_iter().take(count).enumerate() {
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

/// `WAL_FILE_TRACE=1`: the game's failed `CreateFileA/W` opens are logged (path, error), to
/// find the file a game stops on (a dump missing a file, an unreadable one).
fn file_trace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_FILE_TRACE").is_ok_and(|v| v == "1"))
}

fn trace_failed_open(function: &str, path: usize, wide: bool) {
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    let error = unsafe { GetLastError() };
    let text = if path == 0 {
        String::new()
    } else if wide {
        let p = path as *const u16;
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    } else {
        let p = path as *const u8;
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) }).into_owned()
    };
    log!("drive: {function}({text}) failed: error {error}");
    unsafe { SetLastError(error) };
}

macro_rules! hooks {
    ($($idx:literal $name:ident [$conv:ident; $($p:ident: $pt:ty),+] ($($a:ident: $t:ty),*) -> $ret:ty;)*) => {
        $(
            unsafe extern "system" fn $name($($p: $pt,)+ $($a: $t),*) -> $ret {
                $(let $p = $conv($p);)+
                let original: unsafe extern "system" fn($($pt,)+ $($t),*) -> $ret =
                    unsafe { std::mem::transmute(ORIG[$idx].load(Ordering::Relaxed)) };
                let ret = unsafe { original($($p.ptr(),)+ $($a),*) };
                // CreateFileA/W: INVALID_HANDLE_VALUE
                if $idx <= 1 && ret as isize == -1 && file_trace() {
                    trace_failed_open(stringify!($name), [$($p.ptr() as usize),+][0], $idx == 1);
                }
                ret
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
            install();
        }

        /// Installs the redirection of the launcher's cabinet drives (`WAL_DRIVES`), in every
        /// program the payload runs in; a system's own [`init_letter`] adds its drive to it.
        pub fn init_drives() {
            if !mapped().is_empty() {
                install();
            }
        }

        /// The hooks, once (a second IAT hook would take the first one as the original).
        fn install() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                init_game_drive();
                init_mmio(None);
                $(
                    if let Some(o) = unsafe { iat::hook("kernel32.dll", stringify!($name), $name as *const () as usize) } {
                        ORIG[$idx].store(o, Ordering::Relaxed);
                    }
                )*
                for crt in CRT_MODULES {
                    redirect_module(crt);
                }
                // the payload's own files on the cabinet drives (the AMEX board's Y:\amex, the
                // image information E:\ICF1)
                if !mapped().is_empty() {
                    redirect_self();
                }
                profile_hooks();
                if std::env::var("WAL_DRIVE_LATE_DLLS").is_ok_and(|v| v == "1") {
                    $(
                        crate::late_hooks::hook("kernel32.dll", stringify!($name), $name as *const () as usize, &ORIG[$idx]);
                    )*
                    log!("drive: redirect in the game's later DLLs too");
                }
            });
        }

        /// The payload DLL's own file functions (the Rust standard library's kernel32 imports).
        fn redirect_self() {
            use windows_sys::Win32::System::LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW,
            };
            let mut module = std::ptr::null_mut();
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if unsafe { GetModuleHandleExW(flags, redirect_self as *const () as *const u16, &mut module) } == 0 || module.is_null() {
                log!("drive: the payload's own module not found");
                return;
            }
            let base = module as usize;
            $(
                if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", stringify!($name), $name as *const () as usize) } {
                    let _ = ORIG[$idx].compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
                }
            )*
        }

        /// Redirects the file functions of another loaded module (a game DLL opening data
        /// files itself, a C runtime); call after `init`. False when it is not loaded.
        pub fn redirect_module(module: &str) -> bool {
            let name = format!("{module}\0");
            let base = unsafe { GetModuleHandleA(name.as_ptr()) } as usize;
            if base == 0 {
                return false;
            }
            log!("drive: redirecting {module} file functions");
            $(
                if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", stringify!($name), $name as *const () as usize) } {
                    // the module and the game import the same kernel32 function
                    let _ = ORIG[$idx].compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
                }
            )*
            // Winning Eleven 2008's sound.dll opens its banks with mmioOpenA
            init_mmio(Some(base));
            true
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

// ── INI files: the profile functions take their file name last ─────────────────────────────

static ORIG_PROFILE: [AtomicUsize; 10] = [const { AtomicUsize::new(0) }; 10];

unsafe fn profile_orig<F: Copy>(i: usize) -> F {
    let f = ORIG_PROFILE[i].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&f) }
}

unsafe extern "system" fn GetPrivateProfileStringA(app: *const u8, key: *const u8, def: *const u8, out: *mut u8, size: u32, file: *const u8) -> u32 {
    let file = ansi(file);
    let f: unsafe extern "system" fn(*const u8, *const u8, *const u8, *mut u8, u32, *const u8) -> u32 = unsafe { profile_orig(0) };
    let r = unsafe { f(app, key, def, out, size, file.ptr()) };
    if file_trace() {
        let s = |p: *const u8| if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() };
        log!("drive: GetPrivateProfileStringA([{}] {} in {}) -> {:?}", s(app), s(key), s(file.ptr()), s(out));
    }
    r
}
unsafe extern "system" fn GetPrivateProfileStringW(app: *const u16, key: *const u16, def: *const u16, out: *mut u16, size: u32, file: *const u16) -> u32 {
    let file = wide(file);
    let f: unsafe extern "system" fn(*const u16, *const u16, *const u16, *mut u16, u32, *const u16) -> u32 = unsafe { profile_orig(1) };
    unsafe { f(app, key, def, out, size, file.ptr()) }
}
unsafe extern "system" fn GetPrivateProfileIntA(app: *const u8, key: *const u8, def: i32, file: *const u8) -> u32 {
    let file = ansi(file);
    let f: unsafe extern "system" fn(*const u8, *const u8, i32, *const u8) -> u32 = unsafe { profile_orig(2) };
    unsafe { f(app, key, def, file.ptr()) }
}
unsafe extern "system" fn GetPrivateProfileIntW(app: *const u16, key: *const u16, def: i32, file: *const u16) -> u32 {
    let file = wide(file);
    let f: unsafe extern "system" fn(*const u16, *const u16, i32, *const u16) -> u32 = unsafe { profile_orig(3) };
    unsafe { f(app, key, def, file.ptr()) }
}
unsafe extern "system" fn WritePrivateProfileStringA(app: *const u8, key: *const u8, value: *const u8, file: *const u8) -> i32 {
    let file = ansi(file);
    let f: unsafe extern "system" fn(*const u8, *const u8, *const u8, *const u8) -> i32 = unsafe { profile_orig(4) };
    unsafe { f(app, key, value, file.ptr()) }
}
unsafe extern "system" fn WritePrivateProfileStringW(app: *const u16, key: *const u16, value: *const u16, file: *const u16) -> i32 {
    let file = wide(file);
    let f: unsafe extern "system" fn(*const u16, *const u16, *const u16, *const u16) -> i32 = unsafe { profile_orig(5) };
    unsafe { f(app, key, value, file.ptr()) }
}
unsafe extern "system" fn GetPrivateProfileSectionA(app: *const u8, out: *mut u8, size: u32, file: *const u8) -> u32 {
    let file = ansi(file);
    let f: unsafe extern "system" fn(*const u8, *mut u8, u32, *const u8) -> u32 = unsafe { profile_orig(6) };
    unsafe { f(app, out, size, file.ptr()) }
}
unsafe extern "system" fn GetPrivateProfileSectionW(app: *const u16, out: *mut u16, size: u32, file: *const u16) -> u32 {
    let file = wide(file);
    let f: unsafe extern "system" fn(*const u16, *mut u16, u32, *const u16) -> u32 = unsafe { profile_orig(7) };
    unsafe { f(app, out, size, file.ptr()) }
}
unsafe extern "system" fn GetPrivateProfileSectionNamesA(out: *mut u8, size: u32, file: *const u8) -> u32 {
    let file = ansi(file);
    let f: unsafe extern "system" fn(*mut u8, u32, *const u8) -> u32 = unsafe { profile_orig(8) };
    unsafe { f(out, size, file.ptr()) }
}
unsafe extern "system" fn GetPrivateProfileSectionNamesW(out: *mut u16, size: u32, file: *const u16) -> u32 {
    let file = wide(file);
    let f: unsafe extern "system" fn(*mut u16, u32, *const u16) -> u32 = unsafe { profile_orig(9) };
    unsafe { f(out, size, file.ptr()) }
}

/// The INI profile functions redirected too (Super Dragon Ball Heroes' network library reads
/// its settings profiles on X:), in the game and, with `WAL_DRIVE_LATE_DLLS=1`, its DLLs.
fn profile_hooks() {
    let hooks: [(&'static str, usize); 10] = [
        ("GetPrivateProfileStringA", GetPrivateProfileStringA as *const () as usize),
        ("GetPrivateProfileStringW", GetPrivateProfileStringW as *const () as usize),
        ("GetPrivateProfileIntA", GetPrivateProfileIntA as *const () as usize),
        ("GetPrivateProfileIntW", GetPrivateProfileIntW as *const () as usize),
        ("WritePrivateProfileStringA", WritePrivateProfileStringA as *const () as usize),
        ("WritePrivateProfileStringW", WritePrivateProfileStringW as *const () as usize),
        ("GetPrivateProfileSectionA", GetPrivateProfileSectionA as *const () as usize),
        ("GetPrivateProfileSectionW", GetPrivateProfileSectionW as *const () as usize),
        ("GetPrivateProfileSectionNamesA", GetPrivateProfileSectionNamesA as *const () as usize),
        ("GetPrivateProfileSectionNamesW", GetPrivateProfileSectionNamesW as *const () as usize),
    ];
    let late = std::env::var("WAL_DRIVE_LATE_DLLS").is_ok_and(|v| v == "1");
    for (i, (name, f)) in hooks.into_iter().enumerate() {
        if late {
            crate::late_hooks::hook("kernel32.dll", name, f, &ORIG_PROFILE[i]);
        } else if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            ORIG_PROFILE[i].store(o, Ordering::Relaxed);
        }
        if ORIG_PROFILE[i].load(Ordering::Relaxed) == 0 {
            // not imported by the game: the real function for later hooks
            let n = format!("{name}\0");
            let k = unsafe { GetModuleHandleA(c"kernel32.dll".as_ptr().cast()) };
            if let Some(real) = unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(k, n.as_ptr()) } {
                ORIG_PROFILE[i].store(real as usize, Ordering::Relaxed);
            }
        }
    }
}
