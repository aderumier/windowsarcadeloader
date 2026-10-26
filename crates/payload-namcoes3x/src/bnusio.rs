//! Namco USIO board and gun board API (`bnusio.dll`), for Point Blank X (Unity 5.1, Mono).
//!
//! The game's `BNUsioController` (with `Setting.ini`'s `CONTROLLER 0`) calls `bnusio.dll`
//! through Mono `DllImport`s once per frame: `usio_Communication`, the switches
//! (`usio_GetSwIn`), the coin and service counters, then the two guns of the gun board on a COM
//! port (`gun_OpenEx`, `gun_SendCommand(READDATA)`, `gun_GetPositionX/Y`, `gun_IsTriggered`,
//! `gun_IsInRange`). The payload is that DLL (cdecl exports, plain names, as the original's),
//! answered from the virtual sticks:
//!
//! * Switches: test (latching, toggled by `test`; `WAL_NAMCOES3_TEST_TOGGLE=0`: held), service,
//!   menu up / down / enter (operator menu), P1 start (`SW_PUSH17`), P2 start (`SW_PUSH18`).
//! * Coins: each `coin` press adds one to the coin counter, each `service` press one service
//!   credit (the game reads both every frame and decrements them itself).
//! * Guns: the gun position of players 1 and 2 (`lx`/`ly`) as the board's 10-bit values
//!   shifted by 6, y up (the game's default 8-point calibration maps them to the screen as
//!   is); off screen (`offscreen` native, default b6): out of range, no shot.
//! * Crosshairs (`WAL_CROSSHAIR=1`, the common crosshair option; default on): the game's own crosshair is shown
//!   (`GlobalData.isCrosshairVisible`, off on the cabinet), set from `usio_Communication`, on
//!   the game's main thread.
//!
//! At load (`wal-loader`, before the game's entry point): the real keyboard and mouse are
//! filtered (the game reads Escape and, in its simulated controller mode, keys and the mouse),
//! and the cabinet serial number the boot check shows is written in the registry
//! (`HKLM\SOFTWARE\NBGI\SerialNumber`, the cabinet's `sernum_regist_32.exe`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::{Axis, StickState};
use windows_sys::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};

/// Switch bits of `usio_GetSwIn`.
const SW_TEST: u32 = 0x80;
const SW_SERVICE: u32 = 0x4000;
const SW_UP: u32 = 0x2000;
const SW_DOWN: u32 = 0x1000;
const SW_ENTER: u32 = 0x200;
/// `SW_PUSH17` / `SW_PUSH18`: the start buttons of players 1 and 2.
const SW_START: [u32; 2] = [0x0200_0000, 0x0100_0000];
/// The game's accepted firmware versions are 120 to 126.
const FIRMWARE: u16 = 126;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Trigger,
    Offscreen,
    Start,
    Coin,
    Service,
    Test,
    Up,
    Down,
    Enter,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("offscreen", Native::Offscreen),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("service", Native::Service),
    ("test", Native::Test),
    ("up", Native::Up),
    ("down", Native::Down),
    ("enter", Native::Enter),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b6", "offscreen"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
    ("up", "up"),
    ("down", "down"),
    ("b2", "enter"),
];

#[derive(Clone, Copy, Default)]
struct Gun {
    /// board values: 10 bits shifted by 6
    x: u16,
    y: u16,
    trigger: bool,
    in_range: bool,
}

#[derive(Default)]
struct Board {
    switches: u32,
    coins: u16,
    services: u16,
    guns: [Gun; 2],
    /// natives held at the previous `usio_Communication` (edges)
    held: [Vec<Native>; 2],
    test_latched: bool,
    gout: [u8; 32],
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);
static INSTALLED: AtomicBool = AtomicBool::new(false);

fn flag(var: &str, default: bool) -> bool {
    std::env::var(var).map(|v| v.trim() != "0").unwrap_or(default)
}

/// Whether this payload is the game's `bnusio.dll` (Point Blank X), from its file name.
pub(crate) fn installed_as_bnusio(module: *mut c_void) -> bool {
    let mut path = [0u16; 520];
    let n = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32)
    } as usize;
    let path = String::from_utf16_lossy(&path[..n]);
    path.rsplit(['\\', '/']).next().is_some_and(|f| f.eq_ignore_ascii_case("bnusio.dll"))
}

/// At load: the launcher connection, the input filters, the serial number.
pub(crate) fn init() {
    INSTALLED.store(true, Ordering::Relaxed);
    wal_payload_common::start("namcoes3-bnusio");
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize };
    wal_payload_common::input_filter::init(exe, |_| false);
    write_serial();
    log!("namcoes3: USIO and gun board (bnusio.dll) loaded");
}

fn write_serial() {
    let serial = std::env::var("WAL_NAMCOES3_SERIAL").unwrap_or_else(|_| "WAL000000001".into());
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let value = wide(serial.trim());
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(HKEY_LOCAL_MACHINE, wide("SOFTWARE\\NBGI").as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut()) != 0 {
            log!("namcoes3: cannot create HKLM\\SOFTWARE\\NBGI");
            return;
        }
        RegSetValueExW(key, wide("SerialNumber").as_ptr(), 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32);
        RegCloseKey(key);
    }
}

fn maps() -> &'static [ButtonMap<Native>; 2] {
    static MAPS: OnceLock<[ButtonMap<Native>; 2]> = OnceLock::new();
    MAPS.get_or_init(|| [ButtonMap::new(NATIVES, DEFAULTS), ButtonMap::new(NATIVES, DEFAULTS)])
}

/// A gun axis (-32768 ..= 32767) as the board's value, 1 ..= 1023 shifted by 6 (0 is "no
/// position" for the game).
fn gun_axis(v: i16, flip: bool) -> u16 {
    let mut f = (v as f32 + 32768.0) / 65535.0;
    if flip {
        f = 1.0 - f;
    }
    ((f * 1023.0).round().clamp(1.0, 1023.0) as u16) << 6
}

/// Samples the virtual sticks: once per game frame (`usio_Communication`).
fn update(board: &mut Board) {
    let toggle = flag("WAL_NAMCOES3_TEST_TOGGLE", true);
    let mut switches = 0;
    for player in 0..2 {
        let stick: StickState = wal_payload_common::input(player);
        let mut held = Vec::new();
        maps()[player].for_each_pressed(&stick, |n| held.push(n));
        let pressed = |n: Native| held.contains(&n) && !board.held[player].contains(&n);
        if pressed(Native::Coin) {
            board.coins = board.coins.saturating_add(1);
        }
        if pressed(Native::Service) {
            board.services = board.services.saturating_add(1);
        }
        if toggle && pressed(Native::Test) {
            board.test_latched = !board.test_latched;
            log!("namcoes3: test switch {}", if board.test_latched { "on" } else { "off" });
        }
        for n in &held {
            switches |= match n {
                Native::Test if !toggle => SW_TEST,
                Native::Service => SW_SERVICE,
                Native::Up => SW_UP,
                Native::Down => SW_DOWN,
                Native::Enter => SW_ENTER,
                Native::Start => SW_START[player],
                _ => 0,
            };
        }
        board.guns[player] = Gun {
            x: gun_axis(stick.axis(Axis::LeftX), false),
            y: gun_axis(stick.axis(Axis::LeftY), true),
            trigger: held.contains(&Native::Trigger),
            in_range: !held.contains(&Native::Offscreen),
        };
        board.held[player] = held;
    }
    if board.test_latched {
        switches |= SW_TEST;
    }
    board.switches = switches;
}

fn with_board<R>(f: impl FnOnce(&mut Board) -> R) -> R {
    let mut board = BOARD.lock().unwrap();
    f(board.get_or_insert_with(Board::default))
}

fn gun(player: i32) -> Gun {
    match player {
        1 | 2 => with_board(|b| b.guns[player as usize - 1]),
        _ => Gun::default(),
    }
}

/// Shows the game's crosshairs (`Singleton<GlobalData>.instance.isCrosshairVisible`).
fn show_crosshairs() {
    struct Target {
        instance: usize,
        visible: usize,
    }
    static TARGET: OnceLock<Option<Target>> = OnceLock::new();
    let target = TARGET.get_or_init(|| {
        let mono = wal_payload_common::mono::mono()?;
        let image = mono.image("Assembly-CSharp");
        let singleton = mono.class_of_type_name(image, "Artoncode.Core.Singleton`1[[GlobalData, Assembly-CSharp]]");
        let instance = mono.static_addr(singleton, mono.field(singleton, "instance")) as usize;
        let visible = mono.field(mono.class(image, "", "GlobalData"), "isCrosshairVisible");
        if instance == 0 || visible.is_null() {
            log!("namcoes3: GlobalData.isCrosshairVisible not found: no crosshairs");
            return None;
        }
        log!("namcoes3: crosshairs shown");
        Some(Target { instance, visible: mono.offset(visible) })
    });
    if let Some(t) = target {
        let data = unsafe { (t.instance as *const usize).read() };
        if data != 0 {
            unsafe { ((data + t.visible) as *mut u8).write(1) };
        }
    }
}

macro_rules! export {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty $body:block)*) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "C" fn $name($($arg: $ty),*) -> $ret {
                $(let _ = $arg;)*
                $body
            }
        )*
    };
}

export! {
    fn usio_Initialize() -> () {}
    fn usio_Finalize() -> () {}
    fn usio_Open() -> i32 { 0 }
    fn usio_Close() -> i32 { 0 }
    fn usio_Communication(delay: i32) -> i32 {
        if INSTALLED.load(Ordering::Relaxed) {
            with_board(update);
            if wal_payload_common::crosshairs(true) {
                show_crosshairs();
            }
        }
        0
    }
    fn usio_IsConnected() -> i32 { 1 }
    fn usio_ResetIoBoard() -> () {}
    fn usio_SetRegisterU16(address: u16, value: u16) -> i32 { 0 }
    fn usio_SetRegisterU8(address: u16, value: u8) -> i32 { 0 }
    fn usio_SetBuffer(address: u16, buffer: *const u8, length: u16) -> i32 { 0 }
    fn usio_GetRegisterU16(address: u16) -> u16 { 0 }
    fn usio_GetRegisterU8(address: u16) -> u8 { 0 }
    fn usio_GetBuffer(address: u16, buffer: *mut u8, length: u16) -> () {
        if !buffer.is_null() {
            unsafe { std::ptr::write_bytes(buffer, 0, length as usize) };
        }
    }
    fn usio_SetSystemError(bits: u16) -> i32 { 0 }
    fn usio_GetSystemError() -> u16 { 0 }
    fn usio_GetFirmwareVersion() -> u16 { FIRMWARE }
    fn usio_SetExpansionMode(code: u16) -> i32 { 0 }
    fn usio_GetExpansionMode() -> u16 { 0 }
    fn usio_GetIoBoardNameString() -> *const u8 { c"namco ltd.;USIO;Ver1.26".as_ptr().cast() }
    fn usio_GetSwIn() -> u32 { with_board(|b| b.switches) }
    fn usio_GetSwIn64() -> u64 { with_board(|b| b.switches as u64) }
    fn usio_GetAnalogIn(channel: u8) -> u16 { 0 }
    fn usio_SetGout(channel: u8, value: u8) -> i32 {
        with_board(|b| {
            if let Some(g) = b.gout.get_mut(channel as usize) {
                *g = value;
            }
        });
        0
    }
    fn usio_GetGout(channel: u8) -> u8 { with_board(|b| b.gout.get(channel as usize).copied().unwrap_or(0)) }
    fn usio_SetPLCounter(timeout: u16) -> i32 { 0 }
    fn usio_GetCoin(slot: i32) -> u16 { with_board(|b| b.coins) }
    fn usio_GetCoinError(slot: i32) -> u16 { 0 }
    fn usio_GetService(slot: i32) -> u16 { with_board(|b| b.services) }
    fn usio_GetServiceError(slot: i32) -> u16 { 0 }
    fn usio_DecCoin(slot: i32, count: u16) -> i32 {
        with_board(|b| b.coins = b.coins.saturating_sub(count));
        0
    }
    fn usio_DecService(player: i32, count: u16) -> i32 {
        with_board(|b| b.services = b.services.saturating_sub(count));
        0
    }
    fn usio_ResetCoin() -> i32 {
        with_board(|b| {
            b.coins = 0;
            b.services = 0;
        });
        0
    }
    fn usio_SetHopperLimit() -> i32 { 0 }
    fn usio_SetHopperRequest() -> i32 { 0 }
    fn usio_Sram_Read() -> i32 { 0 }
    fn usio_Sram_Write() -> i32 { 0 }
    fn gun_Open(port: u8) -> i32 { 0 }
    fn gun_OpenEx(port: u8, timeout: u32) -> i32 {
        log!("namcoes3: gun board opened (COM{port})");
        0
    }
    fn gun_Close() -> () {}
    fn gun_SendCommand(command: u8) -> i32 { 0 }
    fn gun_GetRomVersion() -> u8 { 1 }
    fn gun_GetPositionX(player: i32) -> u16 { gun(player).x }
    fn gun_GetPositionY(player: i32) -> u16 { gun(player).y }
    fn gun_IsTriggered(player: i32) -> i32 { gun(player).trigger as i32 }
    fn gun_IsInRange(player: i32) -> i32 { gun(player).in_range as i32 }
    fn gun_IsLedChecked(player: i32, led: i32) -> i32 { 1 }
    fn gun_GetLed(player: i32) -> u16 { 0 }
    fn gun_StrikeOnce(player: i32, kind: u8) -> i32 { 0 }
}
