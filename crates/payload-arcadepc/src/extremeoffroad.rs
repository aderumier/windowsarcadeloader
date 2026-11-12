//! Extreme Off Road (LuckSheng's "SurperHummerDemo", Unity 2020.3, IL2CPP, 64-bit):
//! `WAL_ARCADEPC_BOARD=extremeoffroad`.
//!
//! The cabinet's board ("MT", COM1 at 115200 bauds) is reached through the game's
//! `cncommwrapper.dll` (`CreateComm`, `Open`, `Read`, `Write`, `GetInputSize`: `DllImport`s
//! resolved by `GameAssembly.dll`, hooked: `mono_imports.rs`): the payload is that library, no
//! port is opened.
//!
//! - **Board to PC** (`SerialComForMT.ReceiveData` / `DealData`, `CommunicationWithMT.DealData`):
//!   30 bytes, `55 56` (`DealDataHelpper.MThead1/2`), byte 2 the board's version (1), bytes
//!   10..17 `10 11 .. 17`, 18..24 the switches (a cleared bit is pressed), 25 the wheel, 26 the
//!   throttle, 27 the brake (0..255: `GameData.ini`'s calibration, wheel 0 / 128 / 255), 28 the
//!   sum of the bytes before it, 29 `ff`. Switches: byte 21 bit 0 coin, 22 bit 0 start, 1 view,
//!   2 weapon, 3 link, 4 left attack, 5 right attack; 23 bit 0 the seat belt (buckled), 4 up,
//!   5 down, 6 left, 7 right; 24 bit 0 menu, 1 test.
//! - **PC to board**: 20 bytes, `55 56`, 2..9 the lamps, 10..13 the wheel's force feedback
//!   settings, 14..17 the seat's valves and the wheel's motions, 18 the sum, 19 `ff`: accepted.
//!
//! Natives: `start coin view weapon link left_attack right_attack up down left right menu test`;
//! the left stick steers, the pedals throttle and brake.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, mono_imports};
use wal_protocol::Axis;

/// The board's frame length (`DealDataHelpper.MTMCU2PC_DataLength`).
const FRAME: usize = 30;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A switch: its byte and bit.
    Switch(usize, u8),
    Accel,
    Brake,
}

const NATIVES: &[(&str, Native)] = &[
    ("coin", Native::Switch(21, 0)),
    ("start", Native::Switch(22, 0)),
    ("view", Native::Switch(22, 1)),
    ("weapon", Native::Switch(22, 2)),
    ("link", Native::Switch(22, 3)),
    ("left_attack", Native::Switch(22, 4)),
    ("right_attack", Native::Switch(22, 5)),
    ("up", Native::Switch(23, 4)),
    ("down", Native::Switch(23, 5)),
    ("left", Native::Switch(23, 6)),
    ("right", Native::Switch(23, 7)),
    ("menu", Native::Switch(24, 0)),
    ("test", Native::Switch(24, 1)),
    ("accel", Native::Accel),
    ("brake", Native::Brake),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin"),
    ("test", "test"),
    ("service", "menu"),
    ("b1", "weapon"),
    ("b2", "view"),
    ("b3", "left_attack"),
    ("b4", "right_attack"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// A stick or pedal value (-32767..32767) as the board's 0..255.
fn byte(value: i32) -> u8 {
    ((value.clamp(-32767, 32767) + 32767) * 255 / 65534) as u8
}

/// The board's frame: the inputs now.
fn frame() -> [u8; FRAME] {
    let mut f = [0u8; FRAME];
    f[0] = 0x55;
    f[1] = 0x56;
    f[2] = 1;
    for (i, b) in f[10..18].iter_mut().enumerate() {
        *b = 0x10 + i as u8;
    }
    f[18..25].fill(0xff);
    // the seat belt buckled
    f[23] &= !1;
    let stick = input(0);
    let (mut accel, mut brake) = (stick.axis(Axis::Accel).max(0) as i32, stick.axis(Axis::Brake).max(0) as i32);
    let mut wheel = stick.axis(Axis::LeftX) as i32;
    map().for_each_pressed(&stick, |n| match n {
        Native::Switch(at, bit) => f[at] &= !(1 << bit),
        Native::Accel => accel = 32767,
        Native::Brake => brake = 32767,
    });
    // the menus' left / right are the wheel's too
    if f[23] & 0x40 == 0 {
        wheel = -32767;
    } else if f[23] & 0x80 == 0 {
        wheel = 32767;
    }
    f[25] = byte(wheel);
    f[26] = (accel * 255 / 32767) as u8;
    f[27] = (brake * 255 / 32767) as u8;
    f[28] = f[..28].iter().fold(0u8, |s, b| s.wrapping_add(*b));
    f[29] = 0xff;
    f
}

/// The board's bytes not read yet by the game.
static RX: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
static WRITES: AtomicUsize = AtomicUsize::new(0);

/// When the port was opened.
static OPENED: Mutex<Option<Instant>> = Mutex::new(None);

/// The board's bytes, a new frame when the game has read the last one (it reads every 20 ms).
/// Nothing for the first half second after the port opens: `SerialComForMT.OpenSerial` starts
/// its receive thread before it creates the list the frames go to, a frame read before then
/// ends that thread (the board then times out: COM error, no inputs).
fn rx() -> std::sync::MutexGuard<'static, VecDeque<u8>> {
    let mut rx = RX.lock().unwrap();
    let ready = OPENED.lock().unwrap().is_some_and(|t| t.elapsed() >= Duration::from_millis(500));
    if rx.is_empty() && ready {
        rx.extend(frame());
    }
    rx
}

/// The stand-in `cnComm` object.
static COMM: u8 = 0;

extern "C" fn create_comm() -> *const c_void {
    (&raw const COMM).cast()
}

extern "C" fn none(_comm: *const c_void) {}

extern "C" fn yes() -> i32 {
    1
}

extern "C" fn open(_comm: *const c_void, port: i32, _settings: *const u8) -> i32 {
    log!("extremeoffroad: COM{port} opened (the payload's board)");
    *OPENED.lock().unwrap() = Some(Instant::now());
    RX.lock().unwrap().clear();
    1
}

extern "C" fn input_size(_comm: *const c_void) -> i32 {
    rx().len() as i32
}

extern "C" fn read(_comm: *const c_void, buffer: *mut u8, length: i32, _wait: i32) -> i32 {
    let mut rx = rx();
    let n = (length.max(0) as usize).min(rx.len());
    for i in 0..n {
        unsafe { *buffer.add(i) = rx.pop_front().unwrap() };
    }
    n as i32
}

extern "C" fn write(_comm: *const c_void, buffer: *const u8, length: i32, _wait: i32) -> i32 {
    if WRITES.fetch_add(1, Ordering::Relaxed) == 0 {
        let data = unsafe { std::slice::from_raw_parts(buffer, length.max(0) as usize) };
        let hex: Vec<String> = data.iter().map(|b| format!("{b:02x}")).collect();
        log!("extremeoffroad: first frame to the board {}", hex.join(" "));
    }
    length
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module != "cncommwrapper.dll" {
        return None;
    }
    let f = match name {
        "CreateComm" => create_comm as *const () as usize,
        "Open" => open as *const () as usize,
        "Read" => read as *const () as usize,
        "Write" => write as *const () as usize,
        "GetInputSize" => input_size as *const () as usize,
        "IsOpen" | "SetBufferSize" | "SetRTS" => yes as *const () as usize,
        _ => none as *const () as usize,
    };
    Some(f)
}

/// The game's coins per credit (`Settings\GameData.ini`, UTF-16, `Coin_Rate=<n>//Int`): 0 is
/// its free play. `WAL_FREEPLAY` on: 0; off: 1 when it was free. The game reads it at start.
fn free_play() {
    let Some(free) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") else { return };
    let path = "Settings\\GameData.ini";
    let Ok(bytes) = std::fs::read(path) else { return };
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let text = String::from_utf16_lossy(&units);
    let Some(start) = text.find("Coin_Rate=").map(|i| i + "Coin_Rate=".len()) else { return };
    let end = start + text[start..].find("//").unwrap_or(0);
    let rate = text[start..end].trim();
    let wanted = match (free, rate == "0") {
        (true, false) => "0",
        (false, true) => "1",
        _ => return,
    };
    let new = format!("{}{wanted}{}", &text[..start], &text[end..]);
    let out: Vec<u8> = new.encode_utf16().flat_map(u16::to_le_bytes).collect();
    match std::fs::write(path, out) {
        Ok(()) => log!("extremeoffroad: Coin_Rate {rate} -> {wanted}"),
        Err(e) => log!("extremeoffroad: {path} not written: {e}"),
    }
}

pub fn init() {
    free_play();
    mono_imports::hook(resolve);
    log!("extremeoffroad: MT board");
}
