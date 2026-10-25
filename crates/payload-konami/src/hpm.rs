//! Hello! Pop'n Music (JMP): the game is `popn.dll`, run by `wal-avs-boot.exe`; the payload is
//! its `libacio.dll` (the HBHI board: the players' buttons, test, service, coins, security
//! plug; the card reader). spice2x (the open-source reference) has the board's layout.
//!
//! Two players, each an OK button (start) and four buttons, left to right red, blue, yellow,
//! green. The HBHI packet: byte 1 test 0x20, service 0x10, coin mech 0x04; byte 8 player 1's
//! buttons bits 0-3, player 2's bits 4-7; the OK buttons are analog channels 0 (player 1) and 1
//! (player 2), released with the high byte's bit 0 set.
//!
//! popn.dll also needs `kbt.dll`'s `bt_get_ikey_status` (`kbt.rs`; a null call otherwise) and
//! asks the fake DirectInput for IDirectInputJoyConfig8 (refused).
//!
//! Native inputs for `native_map`: `test service coin start button1 button2 button3 button4`
//! (players 1 and 2).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// popn.dll's base, when this process runs it.
pub fn game_module() -> usize {
    let name: Vec<u16> = "popn.dll\0".encode_utf16().collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    /// Buttons 1-4, left to right (red, blue, yellow, green).
    Button(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("button1", Native::Button(0)),
    ("button2", Native::Button(1)),
    ("button3", Native::Button(2)),
    ("button4", Native::Button(3)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "button1"),
    ("b2", "button2"),
    ("b3", "button3"),
    ("b4", "button4"),
];

struct Board {
    maps: [ButtonMap<Native>; 2],
    coin_held: bool,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// The HBHI status packet.
pub(crate) fn packet() -> [u8; 15] {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board {
        maps: [ButtonMap::new(NATIVES, DEFAULT_MAP), ButtonMap::with_defaults(NATIVES, DEFAULT_MAP)],
        coin_held: false,
    });
    let mut raw = [0u8; 15];
    let mut coin = false;
    for player in 0..2 {
        let stick = wal_payload_common::input(player);
        let mut start = false;
        board.maps[player].for_each_pressed(&stick, |n| match n {
            Native::Test => raw[1] |= 0x20,
            Native::Service => raw[1] |= 0x10,
            Native::Coin => {
                raw[1] |= 0x04;
                coin = true;
            }
            Native::Start => start = true,
            Native::Button(i) => raw[8] |= 1 << (player as u8 * 4 + i),
        });
        // analog channel `player`'s high byte
        if !start {
            raw[player * 2] |= 0x01;
        }
    }
    if coin && !board.coin_held {
        crate::hbhi::coin_inserted();
    }
    board.coin_held = coin;
    raw
}

/// `SetCurrentDirectoryW`: the boot's directory kept (the game moves to its data's, where
/// AVS's relative paths no longer resolve; spice2x does the same).
unsafe extern "system" fn set_current_directory(_path: *const u16) -> i32 {
    1
}

type MixerSetControlDetails = unsafe extern "system" fn(usize, *mut std::ffi::c_void, u32) -> u32;
static ORIG_MIXER_SET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `mixerSetControlDetails`: the cabinet's mixer volume set, its failure ignored (Wine's mixer
/// lacks the control the game sets; the boot stops at an error otherwise).
unsafe extern "system" fn mixer_set_control_details(mixer: usize, details: *mut std::ffi::c_void, flags: u32) -> u32 {
    let original = ORIG_MIXER_SET.load(Ordering::Relaxed);
    if original != 0 {
        let original: MixerSetControlDetails = unsafe { std::mem::transmute(original) };
        unsafe { original(mixer, details, flags) };
    }
    0
}

pub fn init(module: *mut std::ffi::c_void) {
    ACTIVE.store(true, Ordering::Relaxed);
    crate::kbt::init(module);
    unsafe { iat::hook("kernel32.dll", "SetCurrentDirectoryW", set_current_directory as *const () as usize) };
    match unsafe { iat::hook("winmm.dll", "mixerSetControlDetails", mixer_set_control_details as *const () as usize) } {
        Some(o) => ORIG_MIXER_SET.store(o, Ordering::Relaxed),
        None => log!("hpm: mixerSetControlDetails import not found"),
    }
    crate::net::adapters_fit();
    crate::icca::set_pin_on_poll();
    eamusement_setting(module);
}

/// The test menu's settings (`dev/nvram/config.dat`, beside the payload: 0x70 bytes, then their
/// MD5): NETWORK OPTIONS' e-AMUSEMENT (u32 at 0x28) set on with an e-Amusement server
/// (`WAL_KONAMI_EASRV=1`, `WAL_KONAMI_SERVICES`), off without (the boot's network check fails
/// otherwise). On, the game takes e-Amusement cards (`game.*`: the server's `easrv/jmp.rs`).
fn eamusement_setting(module: *mut std::ffi::c_void) {
    let mut buf = [0u16; 520];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(module as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    let Some(path) = path.parent().map(|d| d.join("dev").join("nvram").join("config.dat")) else { return };
    let Ok(mut data) = std::fs::read(&path) else {
        log!("hpm: {} not found, e-AMUSEMENT as the game sets it", path.display());
        return;
    };
    if data.len() != 0x80 {
        log!("hpm: {} is {} bytes, e-AMUSEMENT kept", path.display(), data.len());
        return;
    }
    let server = std::env::var("WAL_KONAMI_EASRV").is_ok_and(|v| v.trim() == "1")
        || std::env::var("WAL_KONAMI_SERVICES").is_ok_and(|v| !v.trim().is_empty());
    if (data[0x28] != 0) == server && wal_easrv::kbin::md5(&data[..0x70])[..] == data[0x70..] {
        return;
    }
    data[0x28..0x2c].copy_from_slice(&(server as u32).to_le_bytes());
    let sum = wal_easrv::kbin::md5(&data[..0x70]);
    data[0x70..].copy_from_slice(&sum);
    match std::fs::write(&path, &data) {
        Ok(()) => log!("hpm: e-AMUSEMENT {}", if server { "on" } else { "off" }),
        Err(e) => log!("hpm: {}: {e}", path.display()),
    }
}
