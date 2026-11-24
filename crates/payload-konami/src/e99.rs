//! Bishi Bashi Champ Online (Konami, 2005 build, game code E99): `E99/bin/game.exe` (Direct3D 8)
//! loads `ezusb.dll`, the library of the cabinet's EZ-USB I/O board; the payload is installed as
//! that DLL and answers its `usb*` calls itself.
//!
//! * Boot (`gio_init`, rva 0xbc7b0): `usbStart(0)`, `usbFirmResult()` until 0, `usbCheckAlive()`
//!   true, `usbSecurityInit` / `usbSecurityInitDone`, `usbWdtStart(60)` / `usbWdtStartDone`,
//!   `usbGetPCBID(12 bytes)`: all succeed (0). The security plug: `usbBootSecurity("GQE99")` 0,
//!   then `usbGetKEYID` must fail (its success is error 0x3c), `usbCheckSecurity("GQE99JAA")` 0.
//! * Input (`gio_update`, rva 0xbcde0): `usbPadRead(&u32)` once a frame, active high: DIP
//!   switches SW1-SW4 0x800000 / 0x400000 / 0x200000 / 0x100000, 0x10000 (unused), test 0x40000,
//!   service 0x80000; the three players' start 0x200 / 0x400 / 0x800 and buttons (red, green,
//!   blue) 1 2 4 / 8 0x10 0x20 / 0x40 0x80 0x100. `usbCoinGet(0)`: the coins counted so far.
//! * The dump's `game.exe` has the built-in setting `IS_USE_USBIO` "0" (its other copy,
//!   `game_original.exe`, "1"): without the board the game reads no input at all. It is set
//!   back to "1" in the executable's defaults table (`config.txt`, absent, would override it).
//! * ROM check (`rom_check_thread`, rva 0xc0fb0): every file `D:\romcheck.qsv` lists is hashed,
//!   `ezusb.dll` too (the payload: "the files are broken, reinstall", error 7): the file check's
//!   call (rva 0xc1501) answers true.
//! * The game's files are on the cabinet's drive D: (`D:/E99/data`, `D:/E99/save`): redirected
//!   to the dump root (`WAL_KONAMI_DDRIVE`, default `..\..` from `E99/bin`).
//!
//! Native inputs for `native_map`: `test service coin start red green blue` (players 1-3).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static COINS: AtomicU32 = AtomicU32::new(0);
static LAMPS: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    /// A button: red, green, blue.
    Button(usize),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("red", Native::Button(0)),
    ("green", Native::Button(1)),
    ("blue", Native::Button(2)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "red"),
    ("b2", "green"),
    ("b3", "blue"),
];

const PLAYERS: usize = 3;
/// Each player's start and buttons (red, green, blue).
const START: [u32; PLAYERS] = [0x200, 0x400, 0x800];
const BUTTONS: [[u32; 3]; PLAYERS] = [[0x1, 0x2, 0x4], [0x8, 0x10, 0x20], [0x40, 0x80, 0x100]];
const TEST: u32 = 0x40000;
const SERVICE: u32 = 0x80000;

struct Board {
    maps: Vec<ButtonMap<Native>>,
    coin_held: bool,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

fn pad() -> u32 {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board {
        maps: (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        coin_held: false,
    });
    let mut bits = 0u32;
    let mut coin = false;
    for player in 0..PLAYERS {
        let stick = wal_payload_common::input(player);
        board.maps[player].for_each_pressed(&stick, |n| match n {
            Native::Test => bits |= TEST,
            Native::Service => bits |= SERVICE,
            Native::Coin => coin = true,
            Native::Start => bits |= START[player],
            Native::Button(b) => bits |= BUTTONS[player][b],
        });
    }
    if coin && !board.coin_held {
        COINS.fetch_add(1, Ordering::Relaxed);
    }
    board.coin_held = coin;
    bits
}

/// The game's executable (`game.exe` of the E99 dump).
pub fn detected(module: *mut c_void) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("ezusb.dll")
}

/// `IS_USE_USBIO` in the executable's built-in settings (a 0xa0-byte entry: name, then value at
/// +0x20): "1".
const USBIO_SETTING: usize = 0x1859a0;

fn enable_usbio() {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let size = unsafe { ((exe + ((exe + 0x3c) as *const u32).read_unaligned() as usize + 0x50) as *const u32).read_unaligned() } as usize;
    if size < USBIO_SETTING + 0x22 {
        log!("e99: unknown executable, IS_USE_USBIO kept");
        return;
    }
    let entry = exe + USBIO_SETTING;
    let name = unsafe { std::slice::from_raw_parts(entry as *const u8, 13) };
    if name != b"IS_USE_USBIO\0" {
        log!("e99: unknown executable, IS_USE_USBIO kept");
        return;
    }
    let value = (entry + 0x20) as *mut u8;
    if unsafe { *value } != b'1' {
        let mut old = 0;
        unsafe {
            VirtualProtect(value.cast(), 2, PAGE_READWRITE, &mut old);
            *value = b'1';
            *value.add(1) = 0;
            VirtualProtect(value.cast(), 2, old, &mut old);
        }
        log!("e99: IS_USE_USBIO set to 1 (the I/O board)");
    }
}

/// The ROM check's per-file hash check (`call 0x4dd990`).
const ROM_FILE_CHECK: usize = 0xc1501;

fn skip_rom_check() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + ROM_FILE_CHECK;
    let call = [0xe8, 0x8a, 0xc4, 0x01, 0x00];
    if unsafe { std::slice::from_raw_parts(at as *const u8, 5) } != call {
        log!("e99: unexpected code at the ROM check, not patched");
        return;
    }
    let mov_eax_1 = [0xb8, 0x01, 0x00, 0x00, 0x00];
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(mov_eax_1.as_ptr(), at as *mut u8, 5);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
    log!("e99: ROM check's file hashes skipped");
}

pub fn init() {
    ACTIVE.store(true, Ordering::Relaxed);
    enable_usbio();
    skip_rom_check();
    wal_payload_common::drive::init("WAL_KONAMI_DDRIVE", "..\\..");
    log!("konami: EZ-USB I/O board and security plug emulated (Bishi Bashi Champ Online)");
}

// ezusb.dll: cdecl, 0 = success unless noted

#[unsafe(no_mangle)]
pub extern "C" fn usbStart(_firmware: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbStartWithFile(_file: *const u8) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbEnd() -> i32 {
    0
}

/// The firmware download's state: 0 done.
#[unsafe(no_mangle)]
pub extern "C" fn usbFirmResult() -> i32 {
    0
}

/// True: the board answers.
#[unsafe(no_mangle)]
pub extern "C" fn usbCheckAlive() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn usbGetError() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbPadRead(pad_out: *mut u32) -> i32 {
    if !pad_out.is_null() {
        unsafe { pad_out.write_unaligned(pad()) };
    }
    0
}

/// The coins counted since the start (the game counts the difference).
#[unsafe(no_mangle)]
pub extern "C" fn usbCoinGet(_slot: u32) -> i32 {
    COINS.load(Ordering::Relaxed) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn usbCoinMode(_mode: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbCoinBlocker(_close: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbLamp(lamps: u32) -> i32 {
    if LAMPS.swap(lamps, Ordering::Relaxed) != lamps {
        wal_payload_common::outputs::lamps(lamps as u64, 24);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbMute(_mute: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbGetMute() -> i32 {
    0
}

/// The board's PCB id (12 bytes).
#[unsafe(no_mangle)]
pub extern "C" fn usbGetPCBID(id: *mut u8) -> i32 {
    if !id.is_null() {
        let pcbid = *b"\x01\x20\x10\x00\x00\x00\x00\x01\x01\x01\x00\x00";
        unsafe { std::ptr::copy_nonoverlapping(pcbid.as_ptr(), id, pcbid.len()) };
    }
    0
}

/// The plug's key id: none (the game wants this to fail; error 0x3c otherwise).
#[unsafe(no_mangle)]
pub extern "C" fn usbGetKEYID(_id: *mut u8) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn usbBootSecurity(_code: *const u8, _a: u32, _b: u32, _c: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbCheckSecurity(_code: *const u8, _a: u32, _b: u32, _c: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbSecurityInit() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbSecurityInitDone() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbSetupSecurityComplete() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbWdtStart(_seconds: u32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbWdtStartDone() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbWdtReset() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbWdtStop() -> i32 {
    0
}
