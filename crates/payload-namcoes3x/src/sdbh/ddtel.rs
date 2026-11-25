//! Super Dragon Ball Heroes' network library, `DDTelIFDll.dll`, as the cabinet library
//! (`neodcd.dll`) loads it.
//!
//! On the cabinet the library is the client of a communication service (`tel\DDTelComm.exe`,
//! a 3G / Wi-Fi modem on a serial port) through named pipes and a shared memory table
//! (`TELCOM_API_COMMU_TABLE`). The service fills the table with the operator settings and their
//! profiles; the library's configuration calls read and write them there, and neodcd's
//! settings come from these calls. Without the service the library's other calls fail and
//! neodcd shows error 2-40 (communication processing error).
//!
//! Here the table is made before the library's `initTelIFDll` with each profile's initial
//! value (`CommonSettingProfile.ini`, `GameSettingProfile.ini` of the dump's root): the
//! configuration calls stay the real library's. Every other entry point (telemetry, server
//! game data, card payout reports...) answers 0, success: the game is offline and its hero
//! is kept on the Hero License card ([`super::ic`]). The library's functions are all cdecl.
//!
//! The library's start also needs its log (`TelLog.dll`, the service's traffic logs: left out)
//! and the cabinet's region (`I:\CONF_COMMON\region_mng.dat`, written by the service: region 0
//! when missing).
//!
//! Table layout (0x11EA0 bytes): the common settings block at 0x660, the game settings block
//! at 0x8A80, 0x8420 bytes each: the number of settings (byte) at 0x18, then from 0x20 one
//! entry of 0x160 bytes per setting (96 at most): setting number (byte, +0), update flag
//! (byte, +1), value (i32, +4), free data size (u16, +0x58, 256 at most), free data (text,
//! +0x60, 255 bytes at most).

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA};
use windows_sys::Win32::System::Memory::{CreateFileMappingA, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE};

const TABLE_NAME: &[u8] = b"TELCOM_API_COMMU_TABLE\0";
const TABLE_SIZE: usize = 0x11EA0;
const COMMON_BLOCK: usize = 0x660;
const GAME_BLOCK: usize = 0x8A80;
const BLOCK_SIZE: usize = 0x8420;
const ENTRY: usize = 0x160;
const MAX_ENTRIES: usize = 96;

/// The configuration calls: the real library's, on the table.
const CONFIG: &[&str] = &[
    "getCommonConfigMaster",
    "getCommonConfigPasswordReleaseCnt",
    "getCommonConfigdata",
    "getCommonConfigdataBySetNo",
    "getGameConfigMaster",
    "getGameConfigPasswordReleaseCnt",
    "getGameConfigdata",
    "getGameConfigdataBySetNo",
    "setCommonConfigPasswordRelease",
    "setCommonConfigdata",
    "setCommonConfigdataBySystem",
    "setGameConfigPasswordRelease",
    "setGameConfigdata",
    "setGameConfigdataBySystem",
];

static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static REAL_INIT: AtomicUsize = AtomicUsize::new(0);
/// The table's view, kept mapped for the process's life (0: not made yet).
static TABLE: AtomicUsize = AtomicUsize::new(0);

/// neodcd's `GetProcAddress` hooked: it loads the library itself and looks its functions up.
pub fn init(neodcd: usize) {
    match unsafe { iat::hook_module(neodcd, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("sdbh: neodcd does not import GetProcAddress, no network stand-in"),
    }
}

fn module_name(module: *mut c_void) -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase()
}

unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const u8) -> *const c_void {
    let real: unsafe extern "system" fn(*mut c_void, *const u8) -> *const c_void =
        unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let found = unsafe { real(module, name) };
    // by name only (an ordinal is a small number)
    if name as usize <= 0xFFFF || module_name(module) != "ddtelifdll.dll" {
        return found;
    }
    let name = unsafe { CStr::from_ptr(name.cast()) }.to_string_lossy();
    quiet_log(module as usize);
    if name == "initTelIFDll" {
        REAL_INIT.store(found as usize, Ordering::Relaxed);
        return init_tel as *const c_void;
    }
    if CONFIG.contains(&name.as_ref()) {
        return found;
    }
    success as *const c_void
}

/// The library's log (`TelLog.dll`, its own log files of the service's traffic) left out: its
/// start fails without the service's folders and the library's `initTelIFDll` with it (0x104).
fn quiet_log(library: usize) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| unsafe {
        for name in ["initTelLogLib", "writeTelLogData"] {
            if iat::hook_module(library, "TelLog.dll", name, success as *const () as usize).is_none() {
                log!("sdbh: the network library does not import TelLog.dll!{name}");
            }
        }
        match iat::hook_module(library, "nbamsavdat.dll", "nbamSavdatRead", savdat_read as *const () as usize) {
            Some(o) => SAVDAT_READ.store(o, Ordering::Relaxed),
            None => log!("sdbh: the network library does not import nbamSavdatRead"),
        }
    });
}

static SAVDAT_READ: AtomicUsize = AtomicUsize::new(0);

/// The backup library's read (`nbamSavdatRead(handle, buffer, size, 0, ...)`, 0: read). The
/// service writes the cabinet's region (`I:\CONF_COMMON\region_mng.dat`, 4 bytes) at its
/// first start; without it the library's start fails (0x307). Missing, it is region 0: the
/// settings profiles `CommonSettingProfile.ini` and `GameSettingProfile.ini` (this title's).
unsafe extern "C" fn savdat_read(handle: usize, buffer: *mut u8, size: u32, flags: u32, a: usize, b: usize) -> i32 {
    let real: unsafe extern "C" fn(usize, *mut u8, u32, u32, usize, usize) -> i32 =
        unsafe { std::mem::transmute(SAVDAT_READ.load(Ordering::Relaxed)) };
    let r = unsafe { real(handle, buffer, size, flags, a, b) };
    if r != 0 && size == 4 && !buffer.is_null() {
        unsafe { buffer.cast::<u32>().write_unaligned(0) };
        log!("sdbh: the cabinet's region not saved yet ({r:#x}): region 0, this title's settings profiles");
        return 0;
    }
    r
}

/// Every call of the service: done.
unsafe extern "C" fn success() -> i32 {
    0
}

unsafe extern "C" fn init_tel(path: *const c_char) -> i32 {
    let ini = if path.is_null() { String::new() } else { unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned() };
    build_table();
    let real = REAL_INIT.load(Ordering::Relaxed);
    if real != 0 {
        let f: unsafe extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(real) };
        let r = unsafe { f(path) };
        log!("sdbh: network library initialized ({ini}): configuration loaded ({r:#x}), the communication service is this PC");
    }
    0
}

/// The game's root (the folder above `app\`).
pub(super) fn game_root() -> std::path::PathBuf {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(GetModuleHandleA(std::ptr::null()), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    exe.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_default()
}

fn build_table() {
    if TABLE.load(Ordering::Relaxed) != 0 {
        return;
    }
    let mapping = unsafe {
        CreateFileMappingA(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, TABLE_SIZE as u32, TABLE_NAME.as_ptr())
    };
    if mapping.is_null() {
        return log!("sdbh: network table not made ({})", unsafe { GetLastError() });
    }
    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, TABLE_SIZE) };
    if view.Value.is_null() {
        return log!("sdbh: network table not mapped ({})", unsafe { GetLastError() });
    }
    let table = unsafe { std::slice::from_raw_parts_mut(view.Value as *mut u8, TABLE_SIZE) };
    let root = game_root();
    let common = fill_block(&mut table[COMMON_BLOCK..COMMON_BLOCK + BLOCK_SIZE], &root.join("CommonSettingProfile.ini"));
    // the common free play option: common setting 2 (免费游戏, free play), 0 off / 1 on
    let free = wal_payload_common::free_play("WAL_NAMCOES3_FREEPLAY").unwrap_or(false);
    set_value(&mut table[COMMON_BLOCK..COMMON_BLOCK + BLOCK_SIZE], 2, free as i32);
    log!("sdbh: free play {}", if free { "on" } else { "off" });
    let game = fill_block(&mut table[GAME_BLOCK..GAME_BLOCK + BLOCK_SIZE], &root.join("GameSettingProfile.ini"));
    TABLE.store(view.Value as usize, Ordering::Relaxed);
    log!("sdbh: network table made: {common} common and {game} game settings at their initial values");
}

/// Setting `number`'s value in a block.
fn set_value(block: &mut [u8], number: u8, value: i32) {
    let count = (block[0x18] as usize).min(MAX_ENTRIES);
    for i in 0..count {
        let e = &mut block[0x20 + i * ENTRY..0x20 + (i + 1) * ENTRY];
        if e[0] == number {
            e[4..8].copy_from_slice(&value.to_le_bytes());
        }
    }
}

/// The settings of a profile file in a block; their number.
fn fill_block(block: &mut [u8], path: &std::path::Path) -> usize {
    block.fill(0);
    let Ok(text) = std::fs::read(path) else {
        log!("sdbh: {} not found, no settings", path.display());
        return 0;
    };
    let ini = Ini::parse(&text);
    let profiles = ini.int("COMMON_HEADER", "NumOfProfiles").unwrap_or(0).max(0) as usize;
    let mut count = 0;
    for n in 0..profiles {
        if count == MAX_ENTRIES {
            break;
        }
        let section = format!("SETTING_PROFILE_DETAIL_{n}");
        let Some(number) = ini.int(&section, "SettingNo").filter(|v| *v >= 0) else { continue };
        let e = &mut block[0x20 + count * ENTRY..0x20 + (count + 1) * ENTRY];
        e[0] = number as u8;
        e[1] = ini.int(&section, "InitialUpdateFlg").unwrap_or(0) as u8;
        e[4..8].copy_from_slice(&(ini.int(&section, "InitialSettingValue").unwrap_or(0) as i32).to_le_bytes());
        let size = ini.int(&section, "InitialFreeDataSize").unwrap_or(0).clamp(0, 0x100) as u16;
        e[0x58..0x5A].copy_from_slice(&size.to_le_bytes());
        let data = ini.get(&section, "InitialFreeData").unwrap_or(&[]);
        let len = data.len().min(0xFF);
        e[0x60..0x60 + len].copy_from_slice(&data[..len]);
        count += 1;
    }
    block[0x18] = count as u8;
    count
}

/// An INI file's values, raw bytes (the profiles are in the cabinet's code page).
struct Ini<'a> {
    values: Vec<(&'a [u8], &'a [u8], &'a [u8])>,
}

impl<'a> Ini<'a> {
    fn parse(text: &'a [u8]) -> Self {
        let trim = |s: &'a [u8]| -> &'a [u8] {
            let start = s.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(s.len());
            let end = s.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |e| e + 1);
            &s[start..end.max(start)]
        };
        let mut section: &[u8] = b"";
        let mut values = Vec::new();
        for line in text.split(|&b| b == b'\n') {
            let line = trim(line);
            if line.first() == Some(&b'[') {
                if let Some(end) = line.iter().position(|&b| b == b']') {
                    section = &line[1..end];
                }
            } else if line.first() != Some(&b';')
                && let Some(eq) = line.iter().position(|&b| b == b'=')
            {
                values.push((section, trim(&line[..eq]), trim(&line[eq + 1..])));
            }
        }
        Ini { values }
    }

    fn get(&self, section: &str, key: &str) -> Option<&'a [u8]> {
        self.values
            .iter()
            .find(|(s, k, _)| s.eq_ignore_ascii_case(section.as_bytes()) && k.eq_ignore_ascii_case(key.as_bytes()))
            .map(|v| v.2)
    }

    /// `GetPrivateProfileInt`'s reading: the leading digits (and sign).
    fn int(&self, section: &str, key: &str) -> Option<i64> {
        let v = self.get(section, key)?;
        let (neg, digits) = match v.first() {
            Some(b'-') => (true, &v[1..]),
            _ => (false, v),
        };
        let n = digits.iter().take_while(|b| b.is_ascii_digit()).fold(0i64, |n, b| n * 10 + (b - b'0') as i64);
        Some(if neg { -n } else { n })
    }
}

