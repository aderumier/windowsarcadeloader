//! GTI Club: Supermini Festa!: the payload is the game's `libacio.dll` (the HBHI board: the
//! cabinet's switches, coins, security plug). The steering wheel and its motor are an HGTH board
//! on a second ACIO bus, which gti3.exe reaches through `dllibacio.dll` (another libacio,
//! `LoadLibraryA` + `GetProcAddress`): that load gets this DLL too (`hgth.rs`), one payload
//! instance for both buses.
//!
//! The HBHI packet: byte 1 test 0x20, service 0x10, coin mech 0x04; byte 8 the cabinet's buttons
//! (the test menu's INPUT CHECK): 0x01-0x08 SELECT UP / DOWN / LEFT / RIGHT, 0x10 START, 0x20
//! ACTION, 0x40 / 0x80 the shift lever's SHIFT UP / SHIFT DOWN. The game turns them into its
//! switch flags (0xa7e784, rva 0x41037), next to the wheel's and pedals' own (past half way).
//!
//! Native inputs for `native_map`: `test service coin start action shift_up shift_down
//! select_up select_down select_left select_right`.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static MODULE: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);

/// gti3.exe (the export build, plug GKHGTEAA) or gti3japkey.exe (the Japanese one, GQHGTJAA).
pub fn detected() -> bool {
    crate::file_name(std::ptr::null_mut()).to_ascii_lowercase().starts_with("gti3")
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

type LoadLibraryA = unsafe extern "system" fn(*const u8) -> *mut c_void;

/// `LoadLibraryA("dllibacio.dll")`: this DLL (loaded again by its own path: the game frees it at
/// exit).
unsafe extern "system" fn load_library(name: *const u8) -> *mut c_void {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_LOAD_LIBRARY.load(Ordering::Relaxed)) };
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().eq_ignore_ascii_case(b"dllibacio.dll") {
        let mut buf = [0u16; 260];
        let n = unsafe { GetModuleFileNameW(MODULE.load(Ordering::Relaxed) as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let path = String::from_utf16_lossy(&buf[..n]);
        log!("gti: dllibacio.dll (the HGTH bus) is the payload");
        let mut ansi: Vec<u8> = path.into_bytes();
        ansi.push(0);
        return unsafe { original(ansi.as_ptr()) };
    }
    unsafe { original(name) }
}

/// `GetKeyState`: no PC key down (a debug display reads Shift).
unsafe extern "system" fn get_key_state(_key: i32) -> i16 {
    0
}

/// `GetLogicalDriveStringsA`: C: alone. The boot reads every drive's volume and free space;
/// Wine's Z: (the host's `/`) took over 30 s.
unsafe extern "system" fn get_logical_drive_strings(size: u32, out: *mut u8) -> u32 {
    const DRIVES: &[u8] = b"C:\\\0\0";
    if out.is_null() || (size as usize) < DRIVES.len() {
        return DRIVES.len() as u32;
    }
    unsafe { std::ptr::copy_nonoverlapping(DRIVES.as_ptr(), out, DRIVES.len()) };
    DRIVES.len() as u32 - 1
}

/// Writes `new` at `at` when the code there is `expected`.
unsafe fn patch(what: &str, at: usize, expected: &[u8], new: &[u8]) {
    let current = unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) };
    if current != expected {
        log!("gti: {what}: code differs ({current:02x?}), not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const c_void, new.len(), old, &mut old);
    }
    log!("gti: {what}");
}

pub fn init(module: *mut c_void) {
    ACTIVE.store(true, Ordering::Relaxed);
    MODULE.store(module as usize, Ordering::Relaxed);
    match unsafe { iat::hook("kernel32.dll", "LoadLibraryA", load_library as *const () as usize) } {
        Some(o) => ORIG_LOAD_LIBRARY.store(o, Ordering::Relaxed),
        None => log!("gti: LoadLibraryA import not found, the HGTH bus is the dump's dllibacio.dll"),
    }
    unsafe { iat::hook("user32.dll", "GetKeyState", get_key_state as *const () as usize) };
    unsafe { iat::hook("kernel32.dll", "GetLogicalDriveStringsA", get_logical_drive_strings as *const () as usize) };
    // The boot's I/O calibration check (0x44ba90) wants the wheel's, accelerator's and brake's
    // calibrated flags of the test menu's CALIBRATION (ERROR 5-1550-0000 otherwise): the
    // sticks are scaled onto whatever calibration the game has (`hgth.rs`), passed.
    unsafe {
        patch(
            "boot I/O calibration check passed",
            0x44bad8,
            &[0x80, 0x7c, 0x24, 0x24, 0x00, 0x74, 0x1c],
            &[0xeb, 0x13, 0x90, 0x90, 0x90, 0x90, 0x90],
        )
    };
    // The test menu's GAME APPEARANCE OPTIONS password sets the content level (0x331d71c, saved
    // in the backup, 0..3): KXTY8YP2 1, QH84EVVA 2, GZ6LYJD5 3 (all: the USA track, Money Run,
    // 2 VS 2 car football). Its backup load (0x51c470) answers 3: the saved level, and the fresh
    // backup's (its level byte; the rest was zeroed on entry).
    unsafe {
        patch(
            "content level loaded as 3 (all tracks and modes)",
            0x51c4a0,
            &[0xf3, 0x0f, 0x7e, 0x05, 0x48, 0xd7, 0x31, 0x03, 0x66, 0x0f, 0xd6, 0x07],
            &[0xc7, 0x07, 0x03, 0x00, 0x00, 0x00, 0xc6, 0x47, 0x04, 0x00, 0x90, 0x90],
        );
        patch("fresh backup content level 3", 0x51c4bb, &[0x89, 0x1f, 0x88, 0x5f, 0x04], &[0xc6, 0x07, 0x03, 0x90, 0x90]);
    }
    crate::net::adapters_fit();
    crate::avs::init();
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Sw(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("select_up", Native::Sw(0x01)),
    ("select_down", Native::Sw(0x02)),
    ("select_left", Native::Sw(0x04)),
    ("select_right", Native::Sw(0x08)),
    ("start", Native::Sw(0x10)),
    ("action", Native::Sw(0x20)),
    ("shift_up", Native::Sw(0x40)),
    ("shift_down", Native::Sw(0x80)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("up", "select_up"),
    ("down", "select_down"),
    ("left", "select_left"),
    ("right", "select_right"),
    ("start", "start"),
    ("b1", "shift_up"),
    ("b2", "shift_down"),
    ("b3", "action"),
];

struct Board {
    map: ButtonMap<Native>,
    coin_held: bool,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// The HBHI status packet (no analog channel: the wheel and pedals are the HGTH board's).
pub(crate) fn packet() -> [u8; 15] {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board { map: ButtonMap::new(NATIVES, DEFAULT_MAP), coin_held: false });
    let stick = wal_payload_common::input(0);
    let mut raw = [0u8; 15];
    let mut coin = false;
    board.map.for_each_pressed(&stick, |n| match n {
        Native::Test => raw[1] |= 0x20,
        Native::Service => raw[1] |= 0x10,
        Native::Coin => {
            raw[1] |= 0x04;
            coin = true;
        }
        Native::Sw(bit) => raw[8] |= bit,
    });
    if coin && !board.coin_held {
        crate::hbhi::coin_inserted();
    }
    board.coin_held = coin;
    raw
}
