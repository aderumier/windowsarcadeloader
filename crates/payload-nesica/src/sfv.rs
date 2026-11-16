//! Street Fighter V Type Arcade (Capcom / Taito, NESiCAxLive 2, 64-bit Unreal Engine 4): the
//! cabinet's I/O board is Taito's USB I/O library `UIO_AMIC.dll` (the game's USBIOPlugin, its
//! cdecl exports found with `GetProcAddress`), replaced by this DLL (profile `payload_install`,
//! started by wal-loader: the NESYS pipe and the input filters are up before the game). The
//! game's NESYS client is `NESiCAUtilLib.dll`, talking to the payload's NESYS pipe.
//!
//! `UIO_AMIC_Update(buffer)`: +0x00 the board status (0x200: running), +0x04 the input report's
//! size (0x80), the report from +0x0C: +0x10 / +0x12 the switches (the 16 USBIO keys, from bit 0:
//! lever up, down, left, right, buttons 1-6, START, DUMMY1, TEST, SERVICE, COIN2, COIN1; +0x12
//! holds bits 0-7, +0x10 bits 8-15), +0x11 / +0x13 a second word, active low (+0x13 bit 0 button
//! EX1, bit 1 button EX2, bit 3 a game pad on the board), +0x18 the coin counter (14 bits),
//! +0x34 the NESiCA reader (0x1A: a card read, its 16-character id at +0x3C, the 20-character
//! AMIC code at +0x6C). The game writes an output report at +0x8C (+0x08 its size), the reader's
//! read request in bit 0x40 of +0xB4. One player per cabinet (the versus is cabinet to cabinet).
//!
//! Natives for `WAL_MAP`: `up down left right sw1-sw6 ex1 ex2 start test service coin`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input_filter, log};
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

pub(crate) fn detected(module: HMODULE) -> bool {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().is_some_and(|name| name.eq_ignore_ascii_case("UIO_AMIC.dll"))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Native {
    /// A USBIO switch: its bit in the 16-bit word.
    Switch(u8),
    /// A bit of the second, active low word.
    Low(u8),
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("up", Native::Switch(0)),
    ("down", Native::Switch(1)),
    ("left", Native::Switch(2)),
    ("right", Native::Switch(3)),
    ("sw1", Native::Switch(4)),
    ("sw2", Native::Switch(5)),
    ("sw3", Native::Switch(6)),
    ("sw4", Native::Switch(7)),
    ("sw5", Native::Switch(8)),
    ("sw6", Native::Switch(9)),
    ("ex1", Native::Low(0)),
    ("ex2", Native::Low(1)),
    ("start", Native::Switch(10)),
    ("test", Native::Switch(12)),
    ("service", Native::Switch(13)),
    ("coin", Native::Coin),
];

/// Pad layout (b1-b4 = X Y R1 L1, b5-b8 = A B R2 L2): the punches on the top row (X Y R1),
/// the kicks on the bottom row (A B R2), as the cabinet's two rows of three; EX1 / EX2 on L1 /
/// L2.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "sw1"),
    ("b2", "sw2"),
    ("b3", "sw3"),
    ("b5", "sw4"),
    ("b6", "sw5"),
    ("b7", "sw6"),
    ("b4", "ex1"),
    ("b8", "ex2"),
    ("start", "start"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

pub(crate) fn init() {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    // the real pads: the game's XInput (Unreal's) sees none, only the I/O board plays
    for (ordinal, hook) in [(2, xinput_get_state as *const () as usize), (3, xinput_set_state as *const () as usize)] {
        if unsafe { iat::hook_ordinal(exe, "xinput1_3.dll", ordinal, hook) }.is_none() {
            log!("sfv: XInput ordinal {ordinal} not imported");
        }
    }
    // the real keyboard and mouse: only the launcher's input
    input_filter::init(exe, |_| false);
    title_pack();
    culture();
    free_play(exe);
    log!("sfv: I/O board emulated");
}

/// The NESiCAxLive deliveries, mounted from the executable's drive's root: `\\Patch\\patch.pak`,
/// the `patch_chara_stage` paks, `TitlePlate.pak`... (the profile's `c_folders`). Without them
/// the game reads null objects (the title plates' `/Game/TypeXAsset/TitlePlate/TypeXSpec`
/// at +0x23719DE, the toon shader's data asset at +0x4F5EFD): logged.
fn title_pack() {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = String::from_utf16_lossy(&buf[..n]);
    let drive = exe.get(..2).unwrap_or("C:");
    for pak in ["patch.pak", "TitlePlate.pak"] {
        let path = format!("{drive}\\Patch\\{pak}");
        if !std::path::Path::new(&path).exists() {
            log!("sfv: {path} missing: the dump's Patch folder (NESiCAxLive deliveries) is needed, the game will stop");
        }
    }
}

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

unsafe extern "system" fn xinput_get_state(_index: u32, _state: *mut u8) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

unsafe extern "system" fn xinput_set_state(_index: u32, _vibration: *mut u8) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

/// The common free play option: the system settings' coin count per credit (`CoinCreditCoin`, 0
/// is the test menu's FREE PLAY: `IsFreePlay`, the credit state's +0x316) read as 0 by the
/// settings loader (+0x236A5C6 calls the settings' getter for it: `xor eax, eax` instead). Off:
/// the game's own setting.
fn free_play(exe: usize) {
    if wal_payload_common::free_play("WAL_NESICA_FREEPLAY") != Some(true) {
        return;
    }
    // call +0x2373A30
    const ORIGINAL: [u8; 5] = [0xe8, 0x65, 0x94, 0, 0];
    if unsafe { ((exe + 0x236a5c6) as *const [u8; 5]).read_unaligned() } != ORIGINAL {
        log!("sfv: unknown executable at +0x236a5c6: free play not set");
        return;
    }
    // xor eax, eax; nop dword [rax]
    wal_payload_common::patches::apply_list("236a5c6:31C00F1F00");
    log!("sfv: free play");
}

/// The cabinet's language, Japanese: the saved `Engine.ini`'s `[Internationalization]` culture
/// (`-culture=` is not read). Scaleform's font library is `fonts_<culture>`: the US locale's
/// English one has no Japanese glyph (the menus' texts were empty boxes).
fn culture() {
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return };
    let dir = format!("{local}\\StreetFighterV\\Saved\\Config\\WindowsNoEditor");
    let path = format!("{dir}\\Engine.ini");
    let ini = std::fs::read_to_string(&path).unwrap_or_default();
    let mut section = false;
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.eq_ignore_ascii_case("[Internationalization]");
        } else if section && line.eq_ignore_ascii_case("Culture=ja") {
            return;
        }
    }
    // the section's other culture replaced (the section added when missing)
    let mut out = String::new();
    let mut section = false;
    let mut written = false;
    for line in ini.lines() {
        if section && line.trim().to_ascii_lowercase().starts_with("culture=") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
        if line.trim().starts_with('[') {
            section = line.trim().eq_ignore_ascii_case("[Internationalization]");
            if section {
                out.push_str("Culture=ja\r\n");
                written = true;
            }
        }
    }
    if !written {
        out.push_str("\r\n[Internationalization]\r\nCulture=ja\r\n");
    }
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::write(&path, out) {
        Ok(()) => log!("sfv: culture ja written to {path}"),
        Err(e) => log!("sfv: {path}: {e}"),
    }
}

// ---- UIO_AMIC.dll ----

/// The board's "running" status.
const STATUS_OK: u32 = 0x200;

static COIN_HELD: AtomicBool = AtomicBool::new(false);
static COINS: AtomicU32 = AtomicU32::new(0);
static LAST_SWITCHES: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_Open() -> i32 {
    log!("sfv: UIO_AMIC_Open");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_Close() {}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_GetStatus() -> u32 {
    STATUS_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_GetDeviceVersion() -> i32 {
    100
}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_IsPadMode() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_WdtEnable(_seconds: u16, _name: *const u8) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_WdtDisable() {}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_WdtClear() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_WdtGetDIP() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn UIO_AMIC_WdtPowerOff() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn UIO_AMIC_Update(buffer: *mut u8) -> i32 {
    if buffer.is_null() {
        return 0;
    }
    let Some(map) = MAP.get() else { return 0 };
    let mut switches = 0u16;
    let mut low = 0u16;
    let mut coin = false;
    map.for_each_pressed(&wal_payload_common::input(0), |n| match n {
        Native::Switch(bit) => switches |= 1 << bit,
        Native::Low(bit) => low |= 1 << bit,
        Native::Coin => coin = true,
    });
    // a coin per press on the counter, COIN1 held with it
    if coin {
        switches |= 1 << 15;
        if !COIN_HELD.swap(true, Ordering::Relaxed) {
            COINS.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    let all = u32::from(switches) | u32::from(low) << 16;
    if LAST_SWITCHES.swap(all, Ordering::Relaxed) != all {
        log!("sfv: switches {switches:#06x} {low:#06x}");
    }
    // no game pad on the board (bit 3 high)
    let low = !low;
    let card = crate::rfid::card_inserted();
    unsafe {
        let w32 = |off: usize, v: u32| (buffer.add(off) as *mut u32).write_unaligned(v);
        w32(0, STATUS_OK);
        w32(4, 0x80);
        // the output report: taken
        if (buffer.add(8) as *const u32).read_unaligned() != 0 {
            w32(8, 0);
        }
        *buffer.add(0x10) = (switches >> 8) as u8;
        *buffer.add(0x11) = (low >> 8) as u8;
        *buffer.add(0x12) = switches as u8;
        *buffer.add(0x13) = low as u8;
        (buffer.add(0x18) as *mut u16).write_unaligned((COINS.load(Ordering::Relaxed) & 0x3fff) as u16);
        (buffer.add(0x1a) as *mut u16).write_unaligned(0);
        static OUTPUT: OnceLock<std::sync::Mutex<[u8; 0x40]>> = OnceLock::new();
        let mut output = [0u8; 0x40];
        std::ptr::copy_nonoverlapping(buffer.add(0x8c), output.as_mut_ptr(), 0x40);
        let mut last = OUTPUT.get_or_init(|| std::sync::Mutex::new([0; 0x40])).lock().unwrap();
        if *last != output {
            log!("sfv: output {:02x?}", &output[..]);
            *last = output;
        }
        // the reader reading: the card's moment (WAL_NESICA_CARD_AUTO)
        let read = output[0x28] & 0x40 != 0;
        if read {
            crate::rfid::card_requested();
        }
        *buffer.add(0x34) = if card && read { 0x1a } else { 0 };
        let mut id = [b'0'; 16];
        for (d, c) in id.iter_mut().zip(crate::rfid::card_id().bytes()) {
            *d = c;
        }
        std::ptr::copy_nonoverlapping(id.as_ptr(), buffer.add(0x3c), 16);
        // no AMIC code: a NESiCA (twenty '0's made it an amusement IC card)
        std::ptr::write_bytes(buffer.add(0x6c), 0, 20);
    }
    static CARD: AtomicBool = AtomicBool::new(false);
    if CARD.swap(card, Ordering::Relaxed) != card {
        log!("sfv: NESiCA {}", if card { "on the reader" } else { "removed" });
    }
    0
}
