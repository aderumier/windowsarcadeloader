//! Top Gun (SpiderZSoft's "BiuBiuGun", Unity 2019.3, Mono, 32-bit): the cabinet's dongle and
//! its "SY Fire" I/O board, emulated (`WAL_ARCADEPC_BOARD=syfire`).
//!
//! * Dongle: `IOManager` calls `Dog_Ch.dll` (`DllImport`, cdecl): `open_dog` (0: open) and,
//!   every second, `do_something([0, 1])`, which must answer `[1, 0]`; else the game stops
//!   on a dongle error.
//! * I/O board: `SyFireGiftComm` on COM1 (38400 bauds, Mono's `SerialPort`). The board sends
//!   `B5 B6 len cmd data.. sum` (`len` counts `cmd` and its data, `sum` the bytes before it):
//!   cmd 3 a coin (player, count), 7 the operator keys (a bit each: 0 settings, 1 up / the
//!   left button, 2 replenish, 3 down / the right button, 7 refill), 9 the prize doors, 17 a
//!   shot (player, `x`, `y` in the game window's pixels from its top left, little endian; (0, 0):
//!   off the screen) from its camera gun, 6 tickets paid out (player, count). The PC sends
//!   `A3 A9 len cmd data.. sum` (lamps, cmd 5 tickets to pay out, prize motors, the board's
//!   state), each answered `4F 4B cmd` ("OK"); tickets are reported paid at once.
//!
//! Mono resolves its `DllImport`s with `GetProcAddress`: hooked in `mono-2.0-bdwgc.dll`'s
//! imports (as soon as Unity loads it), the dongle functions and `SerialPort`'s kernel32
//! functions (`payload-common`'s `serial`) are ours.
//!
//! Natives: `trigger offscreen` (player 1's gun), `coin settings left right replenish refill
//! door`.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, iat, input, log, serial};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use crate::demul;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Offscreen,
    Coin,
    Settings,
    Left,
    Right,
    Replenish,
    Refill,
    Door,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("offscreen", Native::Offscreen),
    ("coin", Native::Coin),
    ("settings", Native::Settings),
    ("left", Native::Left),
    ("right", Native::Right),
    ("replenish", Native::Replenish),
    ("refill", Native::Refill),
    ("door", Native::Door),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b6", "offscreen"),
    ("coin", "coin"),
    ("test", "settings"),
    ("up", "left"),
    ("down", "right"),
    ("b2", "left"),
    ("b3", "right"),
    ("start", "replenish"),
];

/// The operator keys' bits (cmd 7).
const KEY_BITS: [(Native, u8); 5] =
    [(Native::Settings, 0), (Native::Left, 1), (Native::Replenish, 2), (Native::Right, 3), (Native::Refill, 7)];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn pressed() -> Vec<Native> {
    let mut natives = Vec::new();
    map().for_each_pressed(&input(0), |n| natives.push(n));
    natives
}

/// A board packet: header, length, command, data, checksum.
fn packet(cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut p = vec![0xB5, 0xB6, data.len() as u8 + 1, cmd];
    p.extend_from_slice(data);
    let sum = p.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    p.push(sum);
    p
}

/// The inputs last sent: the natives held.
static LAST: Mutex<Vec<Native>> = Mutex::new(Vec::new());

/// The board's own data, when the game polls the port: the inputs changed since the last poll.
fn idle() -> Vec<u8> {
    let now = pressed();
    let mut last = LAST.lock().unwrap();
    if *last == now {
        return Vec::new();
    }
    let was = |n| last.contains(&n);
    let is = |n| now.contains(&n);
    let mut out = Vec::new();
    if is(Native::Coin) && !was(Native::Coin) {
        out.extend(packet(3, &[0, 1]));
    }
    let bits = |held: &dyn Fn(Native) -> bool| KEY_BITS.iter().filter(|(n, _)| held(*n)).fold(0u8, |b, (_, bit)| b | 1 << bit);
    let (keys_was, keys_is) = (bits(&was), bits(&is));
    if keys_was != keys_is {
        out.extend(packet(7, &[0, keys_is]));
    }
    if is(Native::Door) != was(Native::Door) {
        out.extend(packet(9, &[0, if is(Native::Door) { 0x33 } else { 0 }]));
    }
    if is(Native::Trigger) && !was(Native::Trigger) {
        let (x, y) = shot_position(is(Native::Offscreen));
        out.extend(packet(17, &[0, x as u8, (x >> 8) as u8, y as u8, (y >> 8) as u8]));
    }
    *last = now;
    out
}

/// Where player 1's gun points in the game window (pixels from its top left); (0, 0) off it.
/// With `WAL_DESKTOP_SIZE`, the game renders full screen on that desktop: its pixels.
fn shot_position(offscreen: bool) -> (u16, u16) {
    let (sx, sy) = demul::on_screen(&input(0), demul::screen_size());
    let area = match desktop::size() {
        Some((w, h)) => {
            let (x, y) = desktop::from_screen(sx as f64, sy as f64);
            Some((x as f32, y as f32, w as f32, h as f32))
        }
        None => demul::client_area(demul::find_window()).map(|(left, top, w, h)| (sx - left, sy - top, w, h)),
    };
    match area {
        Some((x, y, w, h)) if !offscreen && (1.0..w).contains(&x) && (1.0..h).contains(&y) => (x as u16, y as u16),
        _ => (0, 0),
    }
}

/// The board's answer to a packet from the game: "OK" and its command.
fn handler(p: &[u8]) -> Vec<u8> {
    match p {
        [0xA3, 0xA9, _, cmd, ..] => {
            static LOGGED: AtomicU32 = AtomicU32::new(0);
            if LOGGED.fetch_or(1 << (*cmd as u32 % 32), Ordering::Relaxed) & 1 << (*cmd as u32 % 32) == 0 {
                log!("syfire: first command {cmd} {p:02x?}");
            }
            let mut reply = vec![0x4F, 0x4B, *cmd];
            // tickets to pay out (player, count): all paid at once (no dispenser to wait for,
            // else the game shows a ticket error)
            if let [_, _, _, 5, player, count, ..] = p {
                reply.extend(packet(6, &[*player, *count]));
            }
            reply
        }
        _ => Vec::new(),
    }
}

// The dongle (`Dog_Ch.dll`).

extern "C" fn open_dog() -> i32 {
    log!("syfire: dongle opened");
    0
}

extern "C" fn close_dog() -> i32 {
    0
}

extern "C" fn do_something(_buf: *const u8, _len: u32, out: *mut u8, out_len: u32) -> i32 {
    if !out.is_null() && out_len >= 2 {
        unsafe { out.write(1) };
        unsafe { out.add(1).write(0) };
    }
    0
}

/// Mono's `GetProcAddress`.
static ORIG_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_char) -> usize =
        unsafe { std::mem::transmute(ORIG_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    // an ordinal
    if (name as usize) >> 16 == 0 {
        return unsafe { orig(module, name) };
    }
    let wanted = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
    let ours = match wanted {
        "open_dog" => Some(open_dog as *const () as usize),
        "close_dog" => Some(close_dog as *const () as usize),
        "do_something" => Some(do_something as *const () as usize),
        _ if module == unsafe { GetModuleHandleW(windows_sys::w!("kernel32.dll")) } => serial::replacement(wanted),
        _ => None,
    };
    match ours {
        Some(f) => {
            log!("syfire: {wanted} resolved to ours");
            f
        }
        None => unsafe { orig(module, name) },
    }
}

/// Hooks Mono's `GetProcAddress` once Unity has loaded it.
fn hook_mono() {
    std::thread::spawn(|| {
        loop {
            let mono = unsafe { GetModuleHandleW(windows_sys::w!("mono-2.0-bdwgc.dll")) } as usize;
            if mono != 0 {
                match unsafe { iat::hook_module(mono, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
                    Some(o) => {
                        ORIG_GET_PROC_ADDRESS.store(o, Ordering::Relaxed);
                        log!("syfire: Mono's GetProcAddress hooked");
                    }
                    None => log!("syfire: Mono does not import GetProcAddress: no dongle, no board"),
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
}

pub fn init() {
    // the originals of the serial functions (no module's imports to hook)
    serial::install_streaming_in("COM1", handler, idle, &[]);
    let kernel32 = unsafe { GetModuleHandleW(windows_sys::w!("kernel32.dll")) };
    if let Some(f) = unsafe { GetProcAddress(kernel32, c"GetProcAddress".as_ptr().cast()) } {
        ORIG_GET_PROC_ADDRESS.store(f as usize, Ordering::Relaxed);
    }
    hook_mono();
    log!("syfire: SY Fire board on COM1, Dog_Ch dongle");
}
