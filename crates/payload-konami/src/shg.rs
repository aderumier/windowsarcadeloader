//! Silent Hill: The Arcade (GHL, 2007): the cabinet I/O through `shaiolib.dll`.
//!
//! `KSHG.exe` (Direct3D 9) imports the BM-System client `libbms.dll` (the profile installs the
//! payload there, `bms.rs`) and `shaiolib.dll`, the cabinet's I/O library (button board, the
//! two gun boards, the security plug, the card readers, on serial ports): its functions are
//! pointed at the payload (a jump at each one's entry). Every one is cdecl and returns -1 done
//! (true), 0 failed (false).
//!
//! - `shaiolib_BUTTON_status`: 9 bytes, one per switch, 0x80 held, 0x01 changed since the last
//!   read: TEST, SERVICE, the coin mechanism, P1 START, P2 START, P1 OPTION L, P1 OPTION R,
//!   P2 OPTION L, P2 OPTION R.
//! - `shaiolib_GUN_status`: per gun 16 bytes, x and y (i32, the 640x480 screen), a shot since
//!   the last read, off the screen (the shot reloads). The game sees a shot as a trigger press:
//!   one per pull.
//! - `shaiolib_COIN_count` / `COIN_consume`: the coins inserted, then taken.
//! - The security plug: `shaiolib_SECURE_check(_no_error)` is asked each region's plug id
//!   (`GKGHL<region>AA`, Japan `GNGHLJAA`): the one of `WAL_KONAMI_REGION` (default E) is there.
//! - The game's 5-point gun calibration (test menu) is skipped: the pointer is the aim.
//! - The speaker channel (`Data/TestDat/speaker.1`, the test menu's SOUND OPTIONS: 0 2ch, 1
//!   5.1ch, then its CRC-16): without it the boot stops (5-2551-0006 "A speaker channel is
//!   un-setting up"). Written at each boot from `WAL_KONAMI_SPEAKERS` (`2`, default, or `5.1`).
//!
//! Native inputs for `native_map`: `test service coin start trigger offscreen option_l option_r`.

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Trigger,
    Offscreen,
    OptionL,
    OptionR,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("offscreen", Native::Offscreen),
    ("option_l", Native::OptionL),
    ("option_r", Native::OptionR),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "trigger"),
    ("b2", "offscreen"),
    ("b3", "option_l"),
    ("b4", "option_r"),
    ("b6", "offscreen"),
];

/// The players (two guns).
const PLAYERS: usize = 2;

/// The switches in `shaiolib_BUTTON_status`'s order.
const SWITCHES: usize = 9;

struct Io {
    maps: Vec<ButtonMap<Native>>,
    /// The switches held at the last read.
    held: [bool; SWITCHES],
    /// Each gun's trigger (or off screen shot) held at the last read.
    trigger_held: [bool; PLAYERS],
    coin_held: bool,
    coins: u32,
}

static IO: Mutex<Option<Io>> = Mutex::new(None);

fn with_io<R>(f: impl FnOnce(&mut Io) -> R) -> R {
    let mut guard = IO.lock().unwrap();
    let io = guard.get_or_insert_with(|| Io {
        maps: (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        held: [false; SWITCHES],
        trigger_held: [false; PLAYERS],
        coin_held: false,
        coins: 0,
    });
    f(io)
}

fn natives(io: &Io, player: usize, stick: &wal_protocol::StickState) -> Vec<Native> {
    let mut out = Vec::new();
    io.maps[player].for_each_pressed(stick, |n| out.push(n));
    out
}

const TRUE: i32 = -1;
const FALSE: i32 = 0;

unsafe fn put<T>(out: *mut T, value: T) {
    if !out.is_null() {
        unsafe { out.write_unaligned(value) };
    }
}

extern "C" fn yes() -> i32 {
    TRUE
}

extern "C" fn no() -> i32 {
    FALSE
}

extern "C" fn initialize() -> i32 {
    log!("shg: shaiolib_initialize: button board, gun boards and security plug emulated");
    TRUE
}

/// The switches: held 0x80, changed since the last read 0x01.
unsafe extern "C" fn button_status(out: *mut [u8; SWITCHES]) -> i32 {
    let sticks: Vec<_> = (0..PLAYERS).map(wal_payload_common::input).collect();
    let status = with_io(|io| {
        let mut now = [false; SWITCHES];
        let mut coin = false;
        for (player, stick) in sticks.iter().enumerate() {
            for n in natives(io, player, stick) {
                match n {
                    Native::Test => now[0] = true,
                    Native::Service => now[1] = true,
                    Native::Coin => coin = true,
                    Native::Start => now[3 + player] = true,
                    Native::OptionL => now[5 + 2 * player] = true,
                    Native::OptionR => now[6 + 2 * player] = true,
                    Native::Trigger | Native::Offscreen => {}
                }
            }
        }
        if coin && !io.coin_held {
            io.coins += 1;
            log!("shg: coin");
        }
        io.coin_held = coin;
        now[2] = coin;
        let mut status = [0u8; SWITCHES];
        for (i, s) in status.iter_mut().enumerate() {
            *s = if now[i] { 0x80 } else { 0 } | (now[i] != io.held[i]) as u8;
        }
        io.held = now;
        status
    });
    unsafe { put(out, status) };
    TRUE
}

/// A gun's report.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Gun {
    x: i32,
    y: i32,
    shot: i32,
    off: i32,
}

/// Each gun's aim (640x480) and its shot of this read: a trigger pull, off the screen (a reload)
/// when the pointer is outside the window or with `offscreen`.
unsafe extern "C" fn gun_status(out: *mut [Gun; PLAYERS]) -> i32 {
    let sticks: Vec<_> = (0..PLAYERS).map(wal_payload_common::input).collect();
    let guns = with_io(|io| {
        let mut guns = [Gun::default(); PLAYERS];
        for (player, stick) in sticks.iter().enumerate() {
            let pressed = natives(io, player, stick);
            let fx = (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0;
            let fy = (stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0;
            let offscreen = pressed.contains(&Native::Offscreen);
            let on = (0.0..1.0).contains(&fx) && (0.0..1.0).contains(&fy) && !offscreen;
            let trigger = offscreen || pressed.contains(&Native::Trigger);
            let shot = trigger && !io.trigger_held[player];
            io.trigger_held[player] = trigger;
            guns[player] = Gun {
                x: (fx.clamp(0.0, 1.0) * 639.0) as i32,
                y: (fy.clamp(0.0, 1.0) * 479.0) as i32,
                shot: shot as i32,
                off: !on as i32,
            };
        }
        guns
    });
    unsafe { put(out, guns) };
    TRUE
}

unsafe extern "C" fn coin_count(out: *mut u32) -> i32 {
    let coins = with_io(|io| io.coins);
    unsafe { put(out, coins) };
    TRUE
}

extern "C" fn coin_consume(count: u32) -> i32 {
    with_io(|io| io.coins = io.coins.saturating_sub(count));
    TRUE
}

/// The 10-key pad (e-Amusement PIN) and the card readers: nothing pressed, no card.
extern "C" fn nothing_read() -> i32 {
    FALSE
}

/// The plug's region (`REGIONS` index).
static REGION: AtomicUsize = AtomicUsize::new(2);

/// The plug ids the game asks, by region: Japan, Asia, Europe, USA, Korea, Asia (B).
const PLUGS: &[(&str, &CStr)] = &[
    ("J", c"GNGHLJAA"),
    ("A", c"GNGHLAAA"),
    ("E", c"GKGHLEAA"),
    ("U", c"GKGHLUAA"),
    ("K", c"GKGHLKAA"),
    ("B", c"GKGHLABA"),
];

/// Whether the plug is `id`'s.
unsafe extern "C" fn secure_check(id: *const c_char) -> i32 {
    if id.is_null() {
        return FALSE;
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == PLUGS[REGION.load(Ordering::Relaxed)].1 { TRUE } else { FALSE }
}

// The cabinet's ids (hex, the game groups them by 4).

extern "C" fn hardware_id() -> *const c_char {
    c"01401400000100000000".as_ptr()
}

extern "C" fn system_id() -> *const c_char {
    c"0100000800000000".as_ptr()
}

extern "C" fn software_id() -> *const c_char {
    c"0100000900000000".as_ptr()
}

extern "C" fn mother_board() -> *const c_char {
    c"OK".as_ptr()
}

fn overrides() -> Vec<(&'static CStr, usize)> {
    let yes = yes as *const () as usize;
    let no = no as *const () as usize;
    vec![
        (c"shaiolib_initialize", initialize as *const () as usize),
        (c"shaiolib_terminate", yes),
        (c"shaiolib_update", yes),
        (c"shaiolib_start_boot", yes),
        (c"shaiolib_is_boot_all", yes),
        (c"shaiolib_is_boot_board_button", yes),
        (c"shaiolib_is_boot_board_gun", yes),
        (c"shaiolib_is_boot_comport", yes),
        (c"shaiolib_is_error", no),
        (c"shaiolib_get_lasterror", no),
        (c"shaiolib_reset_error", yes),
        (c"shaiolib_BUTTON_status", button_status as *const () as usize),
        (c"shaiolib_GUN_status", gun_status as *const () as usize),
        (c"shaiolib_GUN_enable", yes),
        (c"shaiolib_GUN_disable", yes),
        (c"shaiolib_GUN_flash_on", yes),
        (c"shaiolib_GUN_flash_off", yes),
        (c"shaiolib_GUN_reaction", yes),
        (c"shaiolib_GUN_set_reaction_pattern", yes),
        (c"shaiolib_LED_change", yes),
        (c"shaiolib_COIN_count", coin_count as *const () as usize),
        (c"shaiolib_COIN_consume", coin_consume as *const () as usize),
        (c"shaiolib_COIN_countup", yes),
        (c"shaiolib_COIN_blocker_open", yes),
        (c"shaiolib_COIN_blocker_close", yes),
        (c"shaiolib_10Key_enable", yes),
        (c"shaiolib_10Key_disable", yes),
        (c"shaiolib_10Key_status", nothing_read as *const () as usize),
        (c"shaiolib_CARD_start", yes),
        (c"shaiolib_CARD_stop", yes),
        (c"shaiolib_CARD_status", nothing_read as *const () as usize),
        (c"shaiolib_CARD_status_clear", yes),
        (c"shaiolib_SECURE_check", secure_check as *const () as usize),
        (c"shaiolib_SECURE_check_no_error", secure_check as *const () as usize),
        (c"shaiolib_SECURE_is_renewal", yes),
        (c"shaiolib_SECURE_renew", yes),
        (c"shaiolib_SYSINFO_get_hardware_id", hardware_id as *const () as usize),
        (c"shaiolib_SYSINFO_get_system_id", system_id as *const () as usize),
        (c"shaiolib_SYSINFO_get_software_id", software_id as *const () as usize),
        (c"shaiolib_SYSINFO_get_mother_board", mother_board as *const () as usize),
    ]
}

/// The gun read's calibration: `jne` (calibrated) at 0x4d1303 removed, the board's position is
/// the screen's.
const CALIBRATION: usize = 0x4d1303;
const CALIBRATION_BYTES: [u8; 2] = [0x75, 0x0e];

/// The game's CRC-16 table (CCITT, big-endian, from 0xffff), its settings files' check.
const CRC_TABLE: usize = 0x688330;

/// `Data/TestDat/speaker.1`: the channel byte, then its CRC (high byte first: the CRC of the
/// whole file is 0).
fn write_speaker_channel() {
    let five_one = std::env::var("WAL_KONAMI_SPEAKERS").is_ok_and(|v| v.trim() == "5.1");
    let table = unsafe { std::slice::from_raw_parts(CRC_TABLE as *const u16, 256) };
    if table[1] != 0x1021 {
        log!("shg: unexpected CRC table, speaker channel not set");
        return;
    }
    let value = five_one as u8;
    let crc = (0xffffu16 << 8) ^ table[usize::from(0xffu8 ^ value)];
    let path = "Data\\TestDat\\speaker.1";
    match std::fs::write(path, [value, (crc >> 8) as u8, crc as u8]) {
        Ok(()) => log!("shg: speaker channel {}", if five_one { "5.1ch" } else { "2ch" }),
        Err(e) => log!("shg: {path}: {e}"),
    }
}

/// This payload is the game's libbms.dll, beside shaiolib.dll.
pub fn detected(module: windows_sys::Win32::Foundation::HINSTANCE) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("libbms.dll") && unsafe { !GetModuleHandleA(c"shaiolib.dll".as_ptr().cast()).is_null() }
}

pub fn init() {
    let region = std::env::var("WAL_KONAMI_REGION").unwrap_or_default().trim().to_ascii_uppercase();
    let index = PLUGS.iter().position(|(r, _)| *r == region).unwrap_or(2);
    REGION.store(index, Ordering::Relaxed);
    log!("shg: security plug {}", PLUGS[index].1.to_string_lossy());
    // the game imports both: shaiolib.dll is mapped when libbms's DllMain runs
    let shaiolib = unsafe { GetModuleHandleA(c"shaiolib.dll".as_ptr().cast()) };
    for (name, target) in overrides() {
        match unsafe { GetProcAddress(shaiolib, name.as_ptr().cast()) } {
            Some(f) => unsafe { crate::f24::jump(f as usize, target) },
            None => log!("shg: shaiolib {} not found", name.to_string_lossy()),
        }
    }
    if unsafe { std::slice::from_raw_parts(CALIBRATION as *const u8, 2) } == CALIBRATION_BYTES {
        let mut old = 0;
        unsafe {
            use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
            VirtualProtect(CALIBRATION as *const c_void, 2, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping([0x90u8, 0x90].as_ptr(), CALIBRATION as *mut u8, 2);
            VirtualProtect(CALIBRATION as *const c_void, 2, old, &mut old);
        }
    } else {
        log!("shg: unexpected gun read code, guns aim through the test menu's calibration");
    }
    write_speaker_channel();
    log!("shg: button board, gun boards and security plug emulated (Silent Hill: The Arcade)");
}
