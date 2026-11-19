//! Love Live! School Idol Festival ~after school ACTIVITY~ Next Stage (Square Enix / Taito,
//! NESiCAxLive, Type X4, 2018: `ll3.exe`, 64-bit, Direct3D 11), `WAL_NESICA_GAME=lovelive`. The
//! satellite (rhythm game) cabinet: nine rhythm buttons in an arc, DECIDE / CANCEL, the test
//! panel's SELECT / ENTER, the NESiCA reader.
//!
//! * The I/O board (error 3 without it): a WinUSB device, the interface
//!   {46C3136F-0FD5-4973-BC5A-F4A9F3528139} (`SetupDiGetClassDevsA`, the first one, opened with
//!   `CreateFileA`), its device descriptor VID 0x0AE4 PID 0x0901 bcdDevice 0x0032 (read as the
//!   BCD number 32), bulk pipes 0x81 (IN) and 0x02 (OUT). The game's thread writes a 64-byte report when it has
//!   one, else reads one.
//!   - The read report: byte 6 bits 0-6 the rhythm buttons 1-7, 0x80 ENTER; byte 4: 0x01 / 0x02
//!     buttons 8 / 9, 0x04 CANCEL, 0x08 DECIDE, 0x10 SELECT, 0x20 TEST, 0x40 SERVICE, 0x80 COIN;
//!     bytes 12-13 the coin counter (14 bits), bytes 14-15 the last coin decrement applied (the
//!     game's acknowledgement); byte 0x28 the NESiCA reader (0x11: a card read, its 16-character
//!     id at 0x30).
//!   - The written report: `80 01`, bytes 0x0C-0x0D the coins to take off the counter (sent
//!     until the read report echoes them), 0x10-0x27 the lamps, byte 0x28 the reader (bit 0 on,
//!     bit 6 a read wanted).
//! * COM2 is the Type X4 watchdog (`Tx4Wdt`: `01` status, `05` timeout, `04` start, `01` every
//!   frame): JVS framing, every command answered OK by `rfid.rs`'s port.
//! * Touch (`touched_buttons`): the cabinet has no touch screen (the game reads Windows touches,
//!   but its satellite screens act on none); a touchpad or touchscreen's fingers (the whole pad is
//!   the picture) and the mouse (its left button) press the rhythm button of the note target
//!   they touch, the nine circles of a song's arc.
//! * Free play (the common option): the settings block (`*(*(+0xB4CD78) + 0x20)`, from
//!   `D:\LL3\setting_ls4_satellite.dat`) has free play at +0x5C (the test menu's).
//!
//! Natives for `WAL_MAP`: `btn1`-`btn9 decide cancel select enter test service coin`.

use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, iat, input_filter, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

pub(crate) fn detected() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_NESICA_GAME").is_ok_and(|v| v.trim().eq_ignore_ascii_case("lovelive")))
}

/// The board's switches: their byte of the read report and bit.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Native {
    Switch(u8, u8),
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("btn1", Native::Switch(6, 0x01)),
    ("btn2", Native::Switch(6, 0x02)),
    ("btn3", Native::Switch(6, 0x04)),
    ("btn4", Native::Switch(6, 0x08)),
    ("btn5", Native::Switch(6, 0x10)),
    ("btn6", Native::Switch(6, 0x20)),
    ("btn7", Native::Switch(6, 0x40)),
    ("btn8", Native::Switch(4, 0x01)),
    ("btn9", Native::Switch(4, 0x02)),
    ("enter", Native::Switch(6, 0x80)),
    ("cancel", Native::Switch(4, 0x04)),
    ("decide", Native::Switch(4, 0x08)),
    ("select", Native::Switch(4, 0x10)),
    ("test", Native::Switch(4, 0x20)),
    ("service", Native::Switch(4, 0x40)),
    ("coin", Native::Coin),
];

/// The arc of nine buttons on a pad (b1-b4 = X Y R1 L1, b5-b8 = A B R2 L2): its left half on
/// L2 L1 and the d-pad's left / down, the middle one on the d-pad's up, its right half on the
/// face buttons A B and R1 R2; start DECIDE, the d-pad's right CANCEL; X / Y the test menu's
/// SELECT / ENTER.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("b8", "btn1"),
    ("b4", "btn2"),
    ("left", "btn3"),
    ("down", "btn4"),
    ("up", "btn5"),
    ("b5", "btn6"),
    ("b6", "btn7"),
    ("b3", "btn8"),
    ("b7", "btn9"),
    ("start", "decide"),
    ("right", "cancel"),
    ("b1", "select"),
    ("b2", "enter"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

pub(crate) fn init() {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    board_hooks(exe);
    // the real pads and keyboard: only the I/O board plays
    for (name, hook) in [("XInputGetState", xinput_get_state as *const () as usize), ("XInputSetState", xinput_set_state as *const () as usize)] {
        if unsafe { iat::hook_module(exe, "xinput9_1_0.dll", name, hook) }.is_none() {
            log!("lovelive: {name} not imported");
        }
    }
    input_filter::init(exe, |_| false);
    std::thread::spawn(free_play);
    // "0": no title server (guest play only)
    if std::env::var("WAL_LOVELIVE_SERVER").map_or(true, |v| v.trim() != "0") {
        crate::lovelive_server::start();
    }
    log!("lovelive: I/O board emulated");
}

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

unsafe extern "system" fn xinput_get_state(_index: u32, _state: *mut u8) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

unsafe extern "system" fn xinput_set_state(_index: u32, _vibration: *mut u8) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

// ── free play ────────────────────────────────────────────────────────────────────────────

/// The settings block's pointer (`*(+0xB4CD78) + 0x20`) and its free play field.
const SETTINGS: usize = 0xB4_CD78;
const FREE_PLAY: usize = 0x5C;
/// The free play check's load of the settings pointer (`mov rax, [rip+...]` at +0x2C9188, in
/// the credit display's update +0x2C9150), identifying the executable.
const CHECK: (usize, [u8; 7]) = (0x2C_9188, [0x48, 0x8B, 0x05, 0xE9, 0x3B, 0x88, 0x00]);

/// The common free play option, kept in the settings block (the game loads it after boot, the
/// test menu may write it): off, the game's own setting.
fn free_play() {
    let Some(on) = wal_payload_common::free_play("WAL_NESICA_FREEPLAY") else { return };
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if unsafe { ((exe + CHECK.0) as *const [u8; 7]).read_unaligned() } != CHECK.1 {
        log!("lovelive: unknown executable: free play not set");
        return;
    }
    let mut logged = false;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let root = unsafe { ((exe + SETTINGS) as *const usize).read_volatile() };
        if root == 0 {
            continue;
        }
        let block = unsafe { ((root + 0x20) as *const usize).read_volatile() };
        if block == 0 {
            continue;
        }
        let field = (block + FREE_PLAY) as *mut i32;
        let want = i32::from(on);
        if unsafe { field.read_volatile() } != want {
            unsafe { field.write_volatile(want) };
        }
        if !logged {
            log!("lovelive: free play {}", if on { "on" } else { "off" });
            logged = true;
        }
    }
}

// ── the I/O board ────────────────────────────────────────────────────────────────────────

const GUID: [u8; 16] = [0x6F, 0x13, 0xC3, 0x46, 0xD5, 0x0F, 0x73, 0x49, 0xBC, 0x5A, 0xF4, 0xA9, 0xF3, 0x52, 0x81, 0x39];
const PATH: &str = "\\\\?\\usb#vid_0ae4&pid_0901#wal-ll3io#{46c3136f-0fd5-4973-bc5a-f4a9f3528139}";
/// Fake handles: the device list, the device file, the WinUSB interface.
const DEVINFO: usize = 0x4C4C_3000;
const FILE: usize = 0x4C4C_3004;
const IFACE: usize = 0x4C4C_3008;
const PIPE_IN: u8 = 0x81;
const PIPE_OUT: u8 = 0x02;
const REPORT: usize = 64;

static CLASS_DEVS: AtomicUsize = AtomicUsize::new(0);
static ENUM_INTERFACES: AtomicUsize = AtomicUsize::new(0);
static INTERFACE_DETAIL: AtomicUsize = AtomicUsize::new(0);
static DESTROY_LIST: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE: AtomicUsize = AtomicUsize::new(0);

/// The coin counter (14 bits), the last decrement applied (echoed), the reader's control byte.
static COINS: AtomicU16 = AtomicU16::new(0);
static DECREMENT: AtomicU16 = AtomicU16::new(0);
static READER: AtomicU8 = AtomicU8::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);
/// A player's last button press (the automatic card).
static LAST_PRESS: Mutex<Option<Instant>> = Mutex::new(None);

fn board_hooks(exe: usize) {
    let hooks: &[(&str, &str, usize, &AtomicUsize)] = &[
        ("setupapi.dll", "SetupDiGetClassDevsA", class_devs as *const () as usize, &CLASS_DEVS),
        ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize, &ENUM_INTERFACES),
        ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA", interface_detail as *const () as usize, &INTERFACE_DETAIL),
        ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy_list as *const () as usize, &DESTROY_LIST),
        ("kernel32.dll", "CreateFileA", create_file as *const () as usize, &CREATE_FILE),
    ];
    for &(dll, name, f, slot) in hooks {
        match unsafe { iat::hook_module(exe, dll, name, f) } {
            Some(orig) => slot.store(orig, Ordering::Relaxed),
            None => log!("lovelive: {dll}!{name} not hooked"),
        }
    }
    let winusb: &[(&str, usize)] = &[
        ("WinUsb_Initialize", initialize as *const () as usize),
        ("WinUsb_Free", free as *const () as usize),
        ("WinUsb_GetDescriptor", get_descriptor as *const () as usize),
        ("WinUsb_QueryDeviceInformation", device_information as *const () as usize),
        ("WinUsb_QueryInterfaceSettings", interface_settings as *const () as usize),
        ("WinUsb_QueryPipe", query_pipe as *const () as usize),
        ("WinUsb_ReadPipe", read_pipe as *const () as usize),
        ("WinUsb_WritePipe", write_pipe as *const () as usize),
    ];
    for &(name, f) in winusb {
        if unsafe { iat::hook_module(exe, "winusb.dll", name, f) }.is_none() {
            log!("lovelive: winusb.dll!{name} not hooked");
        }
    }
}

unsafe extern "system" {
    fn SetLastError(code: u32);
    fn SetEvent(event: *mut c_void) -> i32;
}

const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

unsafe extern "system" fn class_devs(guid: *const [u8; 16], enumerator: *const u8, hwnd: usize, flags: u32) -> usize {
    if !guid.is_null() && unsafe { *guid } == GUID {
        return DEVINFO;
    }
    let orig: unsafe extern "system" fn(*const [u8; 16], *const u8, usize, u32) -> usize = unsafe { std::mem::transmute(CLASS_DEVS.load(Ordering::Relaxed)) };
    unsafe { orig(guid, enumerator, hwnd, flags) }
}

unsafe extern "system" fn enum_interfaces(set: usize, info: *mut c_void, guid: *const [u8; 16], index: u32, data: *mut u8) -> i32 {
    if set != DEVINFO {
        let orig: unsafe extern "system" fn(usize, *mut c_void, *const [u8; 16], u32, *mut u8) -> i32 = unsafe { std::mem::transmute(ENUM_INTERFACES.load(Ordering::Relaxed)) };
        return unsafe { orig(set, info, guid, index, data) };
    }
    if index != 0 || data.is_null() {
        unsafe { SetLastError(ERROR_NO_MORE_ITEMS) };
        return 0;
    }
    // SP_DEVICE_INTERFACE_DATA: cbSize, class guid, flags (active), reserved
    unsafe {
        std::ptr::copy_nonoverlapping(GUID.as_ptr(), data.add(4), 16);
        *(data.add(20) as *mut u32) = 1;
    }
    1
}

unsafe extern "system" fn interface_detail(set: usize, data: *mut u8, detail: *mut u8, size: u32, required: *mut u32, info: *mut c_void) -> i32 {
    if set != DEVINFO {
        let orig: unsafe extern "system" fn(usize, *mut u8, *mut u8, u32, *mut u32, *mut c_void) -> i32 = unsafe { std::mem::transmute(INTERFACE_DETAIL.load(Ordering::Relaxed)) };
        return unsafe { orig(set, data, detail, size, required, info) };
    }
    // SP_DEVICE_INTERFACE_DETAIL_DATA_A: cbSize, then the path
    let need = 4 + PATH.len() as u32 + 1;
    if !required.is_null() {
        unsafe { *required = need };
    }
    if detail.is_null() || size < need {
        unsafe { SetLastError(ERROR_INSUFFICIENT_BUFFER) };
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(PATH.as_ptr(), detail.add(4), PATH.len());
        *detail.add(4 + PATH.len()) = 0;
    }
    1
}

unsafe extern "system" fn destroy_list(set: usize) -> i32 {
    if set == DEVINFO {
        return 1;
    }
    let orig: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(DESTROY_LIST.load(Ordering::Relaxed)) };
    unsafe { orig(set) }
}

unsafe extern "system" fn create_file(name: *const u8, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() && unsafe { CStr::from_ptr(name.cast()) }.to_bytes().eq_ignore_ascii_case(PATH.as_bytes()) {
        log!("lovelive: I/O board opened");
        return FILE;
    }
    type CreateFileA = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, usize) -> usize;
    let orig: CreateFileA = unsafe { std::mem::transmute(CREATE_FILE.load(Ordering::Relaxed)) };
    unsafe { orig(name, access, share, sa, disposition, flags, template) }
}

unsafe extern "system" fn initialize(file: usize, iface: *mut usize) -> i32 {
    if file != FILE || iface.is_null() {
        return 0;
    }
    unsafe { *iface = IFACE };
    1
}

unsafe extern "system" fn free(_iface: usize) -> i32 {
    1
}

/// The device descriptor (the game checks its size, VID, PID and bcdDevice).
unsafe extern "system" fn get_descriptor(_iface: usize, kind: u8, _index: u8, _lang: u16, buf: *mut u8, len: u32, transferred: *mut u32) -> i32 {
    const DEVICE: [u8; 18] = [18, 1, 0x00, 0x02, 0, 0, 0, 64, 0xE4, 0x0A, 0x01, 0x09, 0x32, 0x00, 1, 2, 0, 1];
    if kind != 1 || buf.is_null() {
        return 0;
    }
    let n = DEVICE.len().min(len as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(DEVICE.as_ptr(), buf, n);
        if !transferred.is_null() {
            *transferred = n as u32;
        }
    }
    1
}

unsafe extern "system" fn device_information(_iface: usize, _kind: u32, len: *mut u32, buf: *mut u8) -> i32 {
    // DEVICE_SPEED: full speed
    unsafe {
        if !buf.is_null() && !len.is_null() && *len >= 1 {
            *buf = 1;
            *len = 1;
        }
    }
    1
}

unsafe extern "system" fn interface_settings(_iface: usize, _alt: u8, desc: *mut u8) -> i32 {
    // USB_INTERFACE_DESCRIPTOR: 2 endpoints, vendor class
    let d = [9u8, 4, 0, 0, 2, 0xFF, 0, 0, 0];
    unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), desc, d.len()) };
    1
}

unsafe extern "system" fn query_pipe(_iface: usize, _alt: u8, index: u8, info: *mut u8) -> i32 {
    // WINUSB_PIPE_INFORMATION: type (2 bulk), id, max packet size, interval
    let id = match index {
        0 => PIPE_IN,
        1 => PIPE_OUT,
        _ => return 0,
    };
    unsafe {
        std::ptr::write_bytes(info, 0, 12);
        *(info as *mut u32) = 2;
        *info.add(4) = id;
        *(info.add(6) as *mut u16) = 64;
    }
    1
}

/// A completed transfer: the count, the overlapped result and its event.
unsafe fn complete(n: usize, transferred: *mut u32, ov: *mut usize) -> i32 {
    unsafe {
        if !transferred.is_null() {
            *transferred = n as u32;
        }
        if !ov.is_null() {
            *ov = 0; // Internal: STATUS_SUCCESS
            *ov.add(1) = n; // InternalHigh: the count
            let event = *ov.add(3) as *mut c_void;
            if !event.is_null() {
                SetEvent(event);
            }
        }
    }
    1
}

unsafe extern "system" fn write_pipe(_iface: usize, _pipe: u8, buf: *const u8, len: u32, transferred: *mut u32, ov: *mut usize) -> i32 {
    let data = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    if data.len() > 0x28 {
        // the coins to take off: applied once, echoed until the game stops sending them
        let take = u16::from_le_bytes([data[0x0C], data[0x0D]]);
        if take != DECREMENT.load(Ordering::Relaxed) {
            if take != 0 {
                let _ = COINS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some(c.saturating_sub(take)));
                log!("lovelive: {take} coin(s) used");
            }
            DECREMENT.store(take, Ordering::Relaxed);
        }
        let reader = data[0x28];
        if reader != READER.swap(reader, Ordering::Relaxed) {
            log!("lovelive: reader control {reader:#04x}");
        }
    }
    unsafe { complete(data.len(), transferred, ov) }
}

unsafe extern "system" fn read_pipe(_iface: usize, _pipe: u8, buf: *mut u8, len: u32, transferred: *mut u32, ov: *mut usize) -> i32 {
    // the board's report rate (the game's thread reads in a loop)
    std::thread::sleep(Duration::from_millis(4));
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    let report = report();
    let n = report.len().min(out.len());
    out[..n].copy_from_slice(&report[..n]);
    unsafe { complete(n, transferred, ov) }
}

/// The note targets of a song (picture fractions, 1920x1080), along the arc from the top left
/// circle to the top right one, and their rhythm buttons.
const TARGETS: [((f32, f32), u8); 9] = [
    ((0.070, 0.139), 1),
    ((0.102, 0.431), 2),
    ((0.195, 0.681), 3),
    ((0.336, 0.847), 4),
    ((0.500, 0.903), 5),
    ((0.664, 0.847), 6),
    ((0.805, 0.681), 7),
    ((0.898, 0.431), 8),
    ((0.930, 0.139), 9),
];
/// How far from a target's center a touch presses it (picture height fractions).
const TARGET_RADIUS: f32 = 0.11;

/// The rhythm buttons touched (their switch byte and bit): the touch surface's fingers and
/// player 1's mouse or gun while `kp0` is held (its position on the pedal axes, the profile's
/// `input.gun`), each on the nearest note target within reach.
fn touched_buttons() -> Vec<(u8, u8)> {
    let mut points: Vec<(f32, f32)> = wal_payload_common::touches().into_iter().map(|(x, y)| (x as f32 / 65535.0, y as f32 / 65535.0)).collect();
    if wal_payload_common::input(0).pressed(wal_protocol::button::KP0)
        && let Some(at) = gun::pedals_in_picture(0)
    {
        points.push(at);
    }
    points
        .into_iter()
        .filter_map(|(x, y)| {
            TARGETS
                .iter()
                .map(|&((tx, ty), button)| (((x - tx) * 16.0 / 9.0).hypot(y - ty), button))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .filter(|&(d, _)| d < TARGET_RADIUS)
                .map(|(_, button)| if button <= 7 { (6, 1 << (button - 1)) } else { (4, 1 << (button - 8)) })
        })
        .collect()
}

/// The read report from player 1's stick (the cabinet-wide test / service / coin from any).
fn report() -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    r[0] = 0x02;
    r[1] = 0x02;
    // the door switches (not read by the game)
    r[7] = 0xFC;
    let map = MAP.get().expect("lovelive: map");
    let mut coin = false;
    for p in 0..wal_protocol::MAX_PLAYERS {
        let stick = wal_payload_common::input(p);
        map.for_each_pressed(&stick, |n| match n {
            Native::Switch(byte, bit) if p == 0 || matches!(bit, 0x20 | 0x40) && byte == 4 => r[byte as usize] |= bit,
            Native::Coin => coin = true,
            _ => {}
        });
    }
    for (byte, bit) in touched_buttons() {
        r[byte as usize] |= bit;
    }
    if coin {
        r[4] |= 0x80;
    }
    if coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COINS.store((COINS.load(Ordering::Relaxed) + 1) & 0x3FFF, Ordering::Relaxed);
    } else if !coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    r[12..14].copy_from_slice(&COINS.load(Ordering::Relaxed).to_le_bytes());
    r[14..16].copy_from_slice(&DECREMENT.load(Ordering::Relaxed).to_le_bytes());
    // a player's button (DECIDE, CANCEL, a rhythm button): the automatic card's moment
    if r[4] & 0x0F != 0 || r[6] & 0x7F != 0 {
        *LAST_PRESS.lock().unwrap() = Some(Instant::now());
    }
    // the NESiCA reader: a read wanted, a card on the reader (the card input's, or with
    // `WAL_NESICA_CARD_AUTO=1` the player's card within 20 s of a button: the game also polls
    // the reader in its attract, where an always present card started a credit)
    if READER.load(Ordering::Relaxed) & 0x40 != 0 {
        let present = if std::env::var("WAL_NESICA_CARD_AUTO").is_ok_and(|v| v.trim() == "1") {
            LAST_PRESS.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(20))
        } else {
            crate::rfid::card_inserted()
        };
        if present {
            r[0x28] = 0x11;
            for (d, c) in r[0x30..0x40].iter_mut().zip(crate::rfid::card_id().bytes()) {
                *d = c;
            }
        }
    }
    r
}
