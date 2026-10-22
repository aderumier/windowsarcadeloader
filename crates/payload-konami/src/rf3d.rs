//! Road Fighters 3D: the game is `jgt.dll`, run by `wal-avs-boot.exe` (the cabinet launcher's
//! steps); the payload is its `libacio.dll` (the HBHI board: switches, coins, security plug;
//! the card reader). The wheel and pedals are an HGTH board on a second ACIO bus, which jgt.dll
//! loads as `libacioEx.dll` (`LoadLibraryA` + `GetProcAddress`): that load gets this DLL too
//! (`hgth.rs`), one payload instance for both buses. The same engine as GTI Club's.
//!
//! The HBHI packet: byte 1 test 0x20, service 0x10, coin mech 0x04; byte 8 VIEW 0x04, 2D/3D
//! 0x08 and the shift lever's four switches: up 0x10, down 0x20, left 0x40, right 0x80 (the
//! gate of a 6-speed H pattern: 1 up-left, 2 down-left, 3 up, 4 down, 5 up-right, 6
//! down-right).
//!
//! Native inputs for `native_map`: `test service coin view toggle_3d lever_up lever_down
//! lever_left lever_right gear_up gear_down` (`gear_up` / `gear_down` move a sequential gear
//! through the H pattern, neutral below 1st).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static MODULE: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);

/// jgt.dll's base, when this process runs it.
pub fn game_module() -> usize {
    let name: Vec<u16> = "jgt.dll\0".encode_utf16().collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

type LoadLibraryA = unsafe extern "system" fn(*const u8) -> *mut c_void;

/// `LoadLibraryA("libacioEx.dll")`: this DLL (loaded again by its own path).
unsafe extern "system" fn load_library(name: *const u8) -> *mut c_void {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_LOAD_LIBRARY.load(Ordering::Relaxed)) };
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().eq_ignore_ascii_case(b"libacioEx.dll") {
        let mut buf = [0u16; 260];
        let n = unsafe { GetModuleFileNameW(MODULE.load(Ordering::Relaxed) as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let path = String::from_utf16_lossy(&buf[..n]);
        log!("rf3d: libacioEx.dll (the HGTH bus) is the payload");
        let mut ansi: Vec<u8> = path.into_bytes();
        ansi.push(0);
        return unsafe { original(ansi.as_ptr()) };
    }
    unsafe { original(name) }
}

/// `GetKeyState`: no PC key down.
unsafe extern "system" fn get_key_state(_key: i32) -> i16 {
    0
}

/// `GetLogicalDriveStringsA`: C: alone (Wine's Z: is the host's `/`, slow to scan).
unsafe extern "system" fn get_logical_drive_strings(size: u32, out: *mut u8) -> u32 {
    const DRIVES: &[u8] = b"C:\\\0\0";
    if out.is_null() || (size as usize) < DRIVES.len() {
        return DRIVES.len() as u32;
    }
    unsafe { std::ptr::copy_nonoverlapping(DRIVES.as_ptr(), out, DRIVES.len()) };
    DRIVES.len() as u32 - 1
}

pub fn init(module: *mut c_void) {
    ACTIVE.store(true, Ordering::Relaxed);
    MODULE.store(module as usize, Ordering::Relaxed);
    match unsafe { iat::hook("kernel32.dll", "LoadLibraryA", load_library as *const () as usize) } {
        Some(o) => ORIG_LOAD_LIBRARY.store(o, Ordering::Relaxed),
        None => log!("rf3d: LoadLibraryA import not found, the HGTH bus is the dump's libacioEx.dll"),
    }
    unsafe { iat::hook("user32.dll", "GetKeyState", get_key_state as *const () as usize) };
    unsafe { iat::hook("kernel32.dll", "GetLogicalDriveStringsA", get_logical_drive_strings as *const () as usize) };
    crate::net::adapters_fit();
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Sw(u8),
    GearUp,
    GearDown,
}

const LEVER_UP: u8 = 0x10;
const LEVER_DOWN: u8 = 0x20;
const LEVER_LEFT: u8 = 0x40;
const LEVER_RIGHT: u8 = 0x80;

/// The lever's switches in gears 1-6 (neutral: none).
const GEARS: [u8; 6] = [
    LEVER_UP | LEVER_LEFT,
    LEVER_DOWN | LEVER_LEFT,
    LEVER_UP,
    LEVER_DOWN,
    LEVER_UP | LEVER_RIGHT,
    LEVER_DOWN | LEVER_RIGHT,
];

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("view", Native::Sw(0x04)),
    ("toggle_3d", Native::Sw(0x08)),
    ("lever_up", Native::Sw(LEVER_UP)),
    ("lever_down", Native::Sw(LEVER_DOWN)),
    ("lever_left", Native::Sw(LEVER_LEFT)),
    ("lever_right", Native::Sw(LEVER_RIGHT)),
    ("gear_up", Native::GearUp),
    ("gear_down", Native::GearDown),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("up", "lever_up"),
    ("down", "lever_down"),
    ("left", "lever_left"),
    ("right", "lever_right"),
    ("b1", "gear_up"),
    ("b2", "gear_down"),
    ("b3", "view"),
    ("b4", "toggle_3d"),
];

struct Board {
    map: ButtonMap<Native>,
    coin_held: bool,
    /// The sequential gear: 0 neutral, 1-6.
    gear: usize,
    gear_held: (bool, bool),
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// The HBHI status packet (no analog channel: the wheel and pedals are the HGTH board's).
pub(crate) fn packet() -> [u8; 15] {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board {
        map: ButtonMap::new(NATIVES, DEFAULT_MAP),
        coin_held: false,
        gear: 0,
        gear_held: (false, false),
    });
    let stick = wal_payload_common::input(0);
    let mut raw = [0u8; 15];
    let (mut coin, mut up, mut down, mut lever) = (false, false, false, false);
    board.map.for_each_pressed(&stick, |n| match n {
        Native::Test => raw[1] |= 0x20,
        Native::Service => raw[1] |= 0x10,
        Native::Coin => {
            raw[1] |= 0x04;
            coin = true;
        }
        Native::Sw(bit) => {
            raw[8] |= bit;
            lever |= bit >= LEVER_UP;
        }
        Native::GearUp => up = true,
        Native::GearDown => down = true,
    });
    if coin && !board.coin_held {
        crate::hbhi::coin_inserted();
    }
    board.coin_held = coin;
    if lever {
        // the lever moved by hand: the sequential gear starts again from neutral
        board.gear = 0;
    }
    if up && !board.gear_held.0 && board.gear < GEARS.len() {
        board.gear += 1;
    }
    if down && !board.gear_held.1 && board.gear > 0 {
        board.gear -= 1;
    }
    board.gear_held = (up, down);
    if !lever && board.gear > 0 {
        raw[8] |= GEARS[board.gear - 1];
    }
    raw
}
