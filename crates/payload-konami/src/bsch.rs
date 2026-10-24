//! Bishi Bashi Channel (game code R66, `contents/modules/bsch.dll`, AVS2 2.17, 64-bit), run by
//! `wal-avs-boot64.exe`. The payload is installed as `libacio.dll` and `libacio_pix.dll`; the
//! `libacio.dll` copy runs it and points the game's `libacio_pix.dll` imports at its own
//! functions (one state for both).
//!
//! * BMPU board (`libacio.dll`): test and service switches, the coin stock.
//! * PIX rvol board (`libacio_pix.dll`): the four players' red, green and blue buttons and their
//!   disks (a spinning dial each, an 8-bit position).
//! * HDXS (LED board), ICCA card reader: present.
//! * COM1: the LCD monitor (as Winning Eleven 2012's).
//!
//! Both copies export the PIX functions (the game's imports must resolve); the `libacio.dll`
//! copy's are the ones called.
//!
//! Layouts from spice2x (`acio/bmpu`, `acio/pix`, `games/bbc`): the PIX status buffer's button
//! bits (some players' red and blue active low), the disks in bytes 20-23 (players 1, 3, 2, 4).
//!
//! Native inputs for `native_map`: `test service coin red green blue disk_left disk_right`
//! (players 1-4).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::{Axis, MAX_PLAYERS};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Red,
    Green,
    Blue,
    DiskLeft,
    DiskRight,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("red", Native::Red),
    ("green", Native::Green),
    ("blue", Native::Blue),
    ("disk_left", Native::DiskLeft),
    ("disk_right", Native::DiskRight),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("b1", "red"),
    ("b2", "green"),
    ("b3", "blue"),
    // blue starts a game and joins it
    ("start", "blue"),
    ("left", "disk_left"),
    ("right", "disk_right"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// The game DLL.
fn game_module() -> usize {
    let name: Vec<u16> = "bsch.dll\0".encode_utf16().collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

/// This copy is `libacio.dll` beside bsch.dll: it runs the payload.
pub(crate) fn detected(module: crate::HINSTANCE) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("libacio.dll") && game_module() != 0
}

pub(crate) fn init(module: crate::HINSTANCE) {
    wal_payload_common::iat::set_game_module(game_module());
    wal_payload_common::start("konami");
    PAYLOAD.store(module as usize, Ordering::Relaxed);
    for (name, f) in [("GetModuleHandleA", get_module_handle_a as *const () as usize), ("GetModuleHandleW", get_module_handle_w as *const () as usize)] {
        if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            ORIG_GET_MODULE[(name == "GetModuleHandleW") as usize].store(o, Ordering::Relaxed);
        }
    }
    let pix: &[(&str, usize)] = &[
        ("ac_io_pix_begin", ac_io_pix_begin as *const () as usize),
        ("ac_io_pix_begin_get_status", ac_io_pix_begin_get_status as *const () as usize),
        ("ac_io_pix_end", ac_io_pix_end as *const () as usize),
        ("ac_io_pix_end_get_status", ac_io_pix_end_get_status as *const () as usize),
        ("ac_io_pix_get_version", ac_io_pix_get_version as *const () as usize),
        ("ac_io_pix_is_active2", ac_io_pix_is_active2 as *const () as usize),
        ("ac_io_pix_is_active_device", ac_io_pix_is_active_device as *const () as usize),
        ("ac_io_pix_update", ac_io_pix_update as *const () as usize),
        ("ac_io_pix_rvol_change_expand_mode", ac_io_pix_rvol_change_expand_mode as *const () as usize),
        ("ac_io_pix_rvol_control_led_bright", ac_io_pix_rvol_control_led_bright as *const () as usize),
        ("ac_io_pix_rvol_get_control_status_buffer", ac_io_pix_rvol_get_control_status_buffer as *const () as usize),
        ("ac_io_pix_rvol_req_volume_control", ac_io_pix_rvol_req_volume_control as *const () as usize),
        ("ac_io_pix_rvol_update_control_status_buffer", ac_io_pix_rvol_update_control_status_buffer as *const () as usize),
        ("ac_io_pix_rvol_watchdog_off", ac_io_pix_rvol_watchdog_off as *const () as usize),
    ];
    let mut hooked = 0;
    for &(name, f) in pix {
        if unsafe { iat::hook("libacio_pix.dll", name, f) }.is_some() {
            hooked += 1;
        }
    }
    wal_payload_common::serial::install_in("COM1", lcd, &[game_module()]);
    if !std::env::var("WAL_KONAMI_EAMUSEMENT").is_ok_and(|v| v.trim() == "1") {
        eamusement_off();
    }
    log!("konami: BMPU and PIX boards, card reader and LCD emulated (Bishi Bashi Channel, {hooked} PIX imports)");
}

/// e-Amusement off (`WAL_KONAMI_EAMUSEMENT=1` keeps the test menu's setting): with it on, the
/// boot stops on 5-2000-0000 (no router). The test menu's NETWORK OPTIONS are `/cabinet/net`
/// of `dev/nvram/tm_net.xml` (Konami binary XML), its first byte e-amusement; the file is set
/// off before the game reads it. Off, the boot wants no network link (5-1507-0000 "network
/// connection detected", from the PC's IPv4 addresses): that check is skipped.
fn eamusement_off() {
    const PATH: &str = "dev/nvram/tm_net.xml";
    // a0 42 header, <cabinet> (void), <net> (binary): its 32 bytes from offset 0x20
    const HEAD: [u8; 8] = [0xa0, 0x42, 0x00, 0xff, 0x00, 0x00, 0x00, 0x10];
    match std::fs::read(PATH) {
        Ok(mut data) if data.len() >= 0x21 && data[..8] == HEAD && data[0x1c..0x20] == [0, 0, 0, 0x20] => {
            if data[0x20] != 0 {
                data[0x20] = 0;
                match std::fs::write(PATH, &data) {
                    Ok(()) => log!("bsch: e-amusement off (test menu NETWORK OPTIONS)"),
                    Err(e) => log!("bsch: {PATH}: {e}, e-amusement kept"),
                }
            }
        }
        Ok(_) => log!("bsch: {PATH} differs, e-amusement kept"),
        Err(e) => log!("bsch: {PATH}: {e}, e-amusement kept"),
    }
    // the boot's network check (rva 0xdec30: AVS's IPv4 addresses, any not 0.0.0.0 or
    // 169.254.x.x a link): offline, the PC's own network is not the cabinet's LAN
    let at = game_module() + 0xdec83;
    // call 0x1801c1950 (the "network connected" flag)
    let call = [0xe8, 0xc8, 0x2c, 0x0e, 0x00];
    if unsafe { std::slice::from_raw_parts(at as *const u8, 5) } == call {
        let mut old = 0;
        unsafe {
            VirtualProtect(at as *const std::ffi::c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::write_bytes(at as *mut u8, 0x90, 5);
            VirtualProtect(at as *const std::ffi::c_void, 5, old, &mut old);
        }
    } else {
        log!("bsch: network check code differs, the PC's network seen");
    }
}

/// This DLL, answered for the cabinet's boot check libraries.
static PAYLOAD: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_MODULE: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

/// The boot check libraries `kbt.dll` and `kld.dll` (not in the dump): this DLL, which exports
/// their functions the game looks up.
fn boot_check_library(name: &str) -> bool {
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    name.eq_ignore_ascii_case("kbt.dll") || name.eq_ignore_ascii_case("kld.dll")
}

unsafe extern "system" fn get_module_handle_a(name: *const u8) -> usize {
    if !name.is_null() && boot_check_library(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy()) {
        return PAYLOAD.load(Ordering::Relaxed);
    }
    let original: unsafe extern "system" fn(*const u8) -> usize = unsafe { std::mem::transmute(ORIG_GET_MODULE[0].load(Ordering::Relaxed)) };
    unsafe { original(name) }
}

unsafe extern "system" fn get_module_handle_w(name: *const u16) -> usize {
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        if boot_check_library(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) })) {
            return PAYLOAD.load(Ordering::Relaxed);
        }
    }
    let original: unsafe extern "system" fn(*const u16) -> usize = unsafe { std::mem::transmute(ORIG_GET_MODULE[1].load(Ordering::Relaxed)) };
    unsafe { original(name) }
}

/// kbt.dll's `k_bt0002(func, arg)`: runs the game's init `func(arg)` (after the cabinet's
/// checks, none here; spice2x's stub).
#[unsafe(no_mangle)]
pub extern "C" fn k_bt0002(func: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>, arg: *mut std::ffi::c_void) {
    if let Some(f) = func {
        unsafe { f(arg) };
    }
}

/// kbt.dll's dongle (iKey) status: two keys present.
#[unsafe(no_mangle)]
pub extern "C" fn k_bt0001(status: *mut u32) -> i32 {
    bt_get_ikey_status(status)
}

#[unsafe(no_mangle)]
pub extern "C" fn bt_get_ikey_status(status: *mut u32) -> i32 {
    if status.is_null() {
        return 0;
    }
    // two 16-byte entries (spice2x's ikey_status: u32, four bytes, u32, u32): bytes 4 and 6
    // set, the last u32 all ones
    unsafe { std::ptr::write_bytes(status as *mut u8, 0, 32) };
    for i in 0..2 {
        let entry = unsafe { (status as *mut u8).add(i * 16) };
        unsafe {
            *entry.add(4) = 1;
            *entry.add(6) = 1;
            std::ptr::write_unaligned(entry.add(12) as *mut u32, !0);
        }
    }
    0
}

/// The LCD monitor: `0<command> <value>\r\n` echoed as `9<command> <value>\r\n`, then `9OK`.
fn lcd(packet: &[u8]) -> Vec<u8> {
    let Some(command) = packet.strip_prefix(b"0") else { return Vec::new() };
    let command = String::from_utf8_lossy(command);
    let command = command.trim_end_matches(['\r', '\n']);
    let (name, value) = command.split_once(' ').unwrap_or((command, ""));
    format!("9{name} {value}\r\n9OK \r\n").into_bytes()
}

/// What the players' controls hold, read from the virtual sticks.
#[derive(Default, Clone, Copy)]
struct Held {
    test: bool,
    service: bool,
    coin: bool,
    red: [bool; 4],
    green: [bool; 4],
    blue: [bool; 4],
    left: [bool; 4],
    right: [bool; 4],
}

fn held() -> Held {
    let mut h = Held::default();
    for p in 0..MAX_PLAYERS.min(4) {
        let stick = wal_payload_common::input(p);
        map().for_each_pressed(&stick, |n| match n {
            Native::Test => h.test = true,
            Native::Service => h.service = true,
            Native::Coin => h.coin = true,
            Native::Red => h.red[p] = true,
            Native::Green => h.green[p] = true,
            Native::Blue => h.blue[p] = true,
            Native::DiskLeft => h.left[p] = true,
            Native::DiskRight => h.right[p] = true,
        });
    }
    h
}

// ---- BMPU (libacio.dll) ----

static COIN_STOCK: AtomicU32 = AtomicU32::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);
static BMPU: Mutex<[u8; 56]> = Mutex::new([0; 56]);

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_update_control_status_buffer() -> bool {
    let h = held();
    let mut control = 0u32;
    if h.service {
        control |= 0x0f00_0000;
    }
    if h.test {
        control |= 0xf000_0000;
    }
    if h.coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COIN_STOCK.fetch_add(1, Ordering::Relaxed);
    } else if !h.coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    let mut buf = BMPU.lock().unwrap();
    *buf = [0; 56];
    for i in 0..4 {
        buf[i * 4..i * 4 + 4].copy_from_slice(&control.to_le_bytes());
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_get_control_status_buffer(out: *mut u8) -> bool {
    if !out.is_null() {
        let buf = BMPU.lock().unwrap();
        unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_current_coinstock(_slot: u32, stock: *mut u32) -> bool {
    if !stock.is_null() {
        unsafe { *stock = COIN_STOCK.load(Ordering::Relaxed) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_consume_coinstock(_slot: u32, coins: u32) -> bool {
    let stock = COIN_STOCK.load(Ordering::Relaxed);
    COIN_STOCK.store(stock.saturating_sub(coins), Ordering::Relaxed);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_lock_coincounter(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_unlock_coincounter(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_control_coin_blocker_open() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_control_coin_blocker_close() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_control_1p_start_led_on() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_control_1p_start_led_off() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_control_2p_start_led_on() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_control_2p_start_led_off() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_control_led_bright(_leds: u32, _brightness: u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_set_output_mode(_mode: u16) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_bmpu_set_watchdog_time(_time: u8) -> bool {
    true
}

// ---- HDXS (libacio.dll) ----

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_update_control_status_buffer(_node: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_get_control_status_buffer(_node: u32, out: *mut u8) -> bool {
    if !out.is_null() {
        unsafe { std::ptr::write_bytes(out, 0, 32) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_led_set_pattern(_index: u32, _r: u8, _g: u8, _b: u8, _leds: u64) -> bool {
    true
}

// ---- PIX rvol (libacio_pix.dll) ----

static PIX: Mutex<[u8; 60]> = Mutex::new([0; 60]);
/// The disks' positions (players 1-4).
static DISKS: [AtomicU8; 4] = [const { AtomicU8::new(0) }; 4];

/// The status buffer's bit of each player's red, green and blue button, and whether it is set
/// when the button is up (active low).
const BUTTONS: [[(usize, bool); 3]; 4] = [
    [(44, false), (107, false), (41, false)],
    [(39, true), (123, false), (55, true)],
    [(71, true), (104, false), (87, true)],
    [(103, true), (42, false), (119, true)],
];

/// A disk's turn per status update: the left stick's full tilt, or the left / right controls.
const DISK_SPEED: f32 = 12.0;

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_rvol_update_control_status_buffer() -> bool {
    let h = held();
    let mut buf = [0u8; 60];
    for p in 0..4 {
        for (i, &(bit, low)) in BUTTONS[p].iter().enumerate() {
            let pressed = [h.red[p], h.green[p], h.blue[p]][i];
            if pressed != low {
                buf[bit / 8] |= 1 << (bit % 8);
            }
        }
        let tilt = wal_payload_common::input(p).axis(Axis::LeftX) as f32 / 32767.0;
        let mut turn = if tilt.abs() > 0.2 { tilt * DISK_SPEED } else { 0.0 };
        if h.left[p] {
            turn -= DISK_SPEED;
        }
        if h.right[p] {
            turn += DISK_SPEED;
        }
        let d = &DISKS[p];
        d.store(d.load(Ordering::Relaxed).wrapping_add(turn as i8 as u8), Ordering::Relaxed);
    }
    // the disks in players 1, 3, 2, 4 order
    for (i, p) in [0, 2, 1, 3].into_iter().enumerate() {
        buf[20 + i] = DISKS[p].load(Ordering::Relaxed);
    }
    *PIX.lock().unwrap() = buf;
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_rvol_get_control_status_buffer(out: *mut u8) -> *mut u8 {
    if !out.is_null() {
        let buf = PIX.lock().unwrap();
        unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_begin(_a: u8, _b: i64, _c: i32, _d: i32, _e: i32, _f: i32) -> bool {
    log!("bsch: PIX bus opened");
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_begin_get_status(_a: i32, _b: i32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_end(_a: i32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_end_get_status(_a: i32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_get_version(_out: *mut u8, _a: i32, _b: i32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_is_active2(_a: i32, status: *mut i32, _b: i32) -> bool {
    if !status.is_null() {
        unsafe { *status = 6 };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_is_active_device(device: i32, _a: i32) -> bool {
    device != 5
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_update(_a: i64) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_rvol_change_expand_mode(_mode: u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_rvol_control_led_bright(_leds: u32, _brightness: u8) -> i64 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_rvol_req_volume_control(_a: u8, _b: u8, _c: u8, _d: u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_pix_rvol_watchdog_off() {}

