//! Taito FastIO emulation: the `iDmacDrv*` exports of `iDmacDrv32.dll`.
//!
//! The game polls 32-bit registers; inputs come from the virtual arcade sticks,
//! mapped to the native FastIO buttons (see `NATIVES` / `DEFAULT_MAP`).

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState};

/// Native inputs of one FastIO player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    Up,
    Down,
    Left,
    Right,
    Start,
    Coin,
    Service,
    Test,
    Btn(u8),
    Ext(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("service", Native::Service),
    ("test", Native::Test),
    ("btn1", Native::Btn(1)),
    ("btn2", Native::Btn(2)),
    ("btn3", Native::Btn(3)),
    ("btn4", Native::Btn(4)),
    ("btn5", Native::Btn(5)),
    ("btn6", Native::Btn(6)),
    ("ext1", Native::Ext(1)),
    ("ext2", Native::Ext(2)),
    ("ext3", Native::Ext(3)),
];

/// Virtual stick -> FastIO. Games override it with `WAL_MAP` (e.g. Arcana Heart 2: `b5=btn6`).
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "btn1"),
    ("b2", "btn2"),
    ("b3", "btn3"),
    ("b4", "btn4"),
    ("b5", "btn5"),
    ("b6", "btn6"),
];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static COIN_HELD: AtomicBool = AtomicBool::new(false);

pub(crate) fn init() {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
}

/// Native FastIO input block, same layout as WindowsLoader's FastIO pipe:
/// bytes 0..=3 players 1/2, byte 4 coin, bytes 8/9 analogs, bytes 10..=14 players 3/4.
fn native_state() -> [u8; 16] {
    let map = MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    let mut d = [0u8; 16];
    for player in 0..wal_protocol::MAX_PLAYERS {
        let stick = wal_payload_common::input(player);
        // players 1/2 share bytes 0..=3 (odd player on the high bit of each pair),
        // players 3/4 the same layout from byte 10
        let base = if player < 2 { 0 } else { 10 };
        let shift = (player % 2) as u32;
        map.for_each_pressed(&stick, |n| {
            let (byte, bit) = match n {
                Native::Start => (0, 0x10),
                Native::Service => (0, 0x04),
                Native::Test => (0, 0x40),
                Native::Ext(1) => (0, 0x01),
                Native::Ext(2) => (0, 0x02),
                Native::Ext(_) => (0, 0x80),
                Native::Up => (1, 0x01),
                Native::Down => (1, 0x04),
                Native::Left => (1, 0x10),
                Native::Right => (1, 0x40),
                Native::Btn(b @ 1..=4) => (2, 0x01 << ((b - 1) * 2)),
                Native::Btn(b) => (3, 0x01 << ((b - 5) * 2)),
                Native::Coin => {
                    d[if player < 2 { 4 } else { 14 }] = 1;
                    return;
                }
            };
            // test/ext are cabinet-wide: same bit whatever the player
            let bit = if matches!(n, Native::Test | Native::Ext(_)) { bit } else { bit << shift };
            d[base + byte] |= bit as u8;
        });
    }
    let p1: StickState = wal_payload_common::input(0);
    d[8] = p1.axis_u8(Axis::LeftX);
    d[9] = p1.axis_u8(Axis::Accel);
    d
}

fn coin_edge(pressed: bool) -> u32 {
    // the game counts one coin per read returning 1: report each press once
    if pressed {
        if !COIN_HELD.swap(true, Ordering::Relaxed) {
            return 1;
        }
    } else {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    0
}

fn le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

unsafe fn put(ptr: *mut c_void, value: u32) {
    if !ptr.is_null() {
        unsafe { *(ptr as *mut u32) = value };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn iDmacDrvOpen(device_id: i32, out: *mut c_void, flag: *mut c_void) -> u32 {
    log!("fastio: open device {device_id}");
    unsafe {
        put(out, 284);
        put(flag, 0);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn iDmacDrvClose(_device_id: i32, _write_access: *mut c_void) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn iDmacDrvRegisterRead(
    _device_id: i32,
    command: u32,
    out: *mut c_void,
    result: *mut c_void,
) -> i32 {
    let value = match command {
        0x400 => 0x0001_0201,
        0x4000 => 0x00FF_00FF,
        0x4004 => 0x00FF_0000,
        0x4120 => le(&native_state()[0..4]),
        0x4124 | 0x41A4 => 0x0110_0000,
        0x4128 => {
            let d = native_state();
            d[8] as u32 | (d[9] as u32) << 8
        }
        0x4140 => coin_edge(native_state()[4] != 0),
        0x4144 | 0x41C4 => native_state()[5] as u32,
        0x41A0 => le(&native_state()[10..14]),
        0x4150 => 0x1823C,
        _ => 0,
    };
    trace("read", command, value);
    unsafe {
        put(out, value);
        put(result, 0);
    }
    0
}

/// Logs each command the first time, and every change of an input register.
fn trace(kind: &str, command: u32, value: u32) {
    static SEEN: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap();
    match seen.iter_mut().find(|(c, _)| *c == command) {
        None => {
            seen.push((command, value));
            log!("fastio: {kind} {command:#06x} -> {value:#010x} (first)");
        }
        Some((_, last)) if *last != value => {
            *last = value;
            if matches!(command, 0x4120 | 0x4128 | 0x4140 | 0x41A0) {
                log!("fastio: {kind} {command:#06x} -> {value:#010x}");
            }
        }
        _ => {}
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn iDmacDrvRegisterWrite(
    _device_id: i32,
    command: u32,
    _value: i32,
    result: *mut c_void,
) -> i32 {
    trace("write", command, _value as u32);
    let ack = matches!(command, 0x4000 | 0x4004 | 0x4100 | 0x4180 | 0x4184 | 0x4188 | 0x418C)
        || (0x4101..=0x410C).contains(&command);
    if ack {
        unsafe { put(result, u32::MAX) };
    }
    0
}

macro_rules! stub_exports {
    ($($name:ident($($arg:ident),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: usize),*) -> i32 {
            $(let _ = $arg;)*
            trace(stringify!($name), 0, 0);
            0
        }
    )*};
}

stub_exports! {
    iDmacDrvDmaRead(a, b, c, d);
    iDmacDrvDmaWrite(a, b, c, d);
    iDmacDrvRegisterBufferRead(a, b, c, d, e);
    iDmacDrvRegisterBufferWrite(a, b, c, d, e);
    iDmacDrvMemoryRead(a, b, c, d);
    iDmacDrvMemoryWrite(a, b, c, d);
    iDmacDrvMemoryReadExt(a, b, c, d, e, f);
    iDmacDrvMemoryWriteExt(a, b, c, d, e, f);
    iDmacDrvMemoryBufferRead(a, b, c, d, e);
    iDmacDrvMemoryBufferWrite(a, b, c, d, e);
}
