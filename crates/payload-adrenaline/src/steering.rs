//! The steering board of Need for Speed Heat Takedown (Sarbakan's "AAG-04-0001" board: the
//! wheel with its motor, the pedals, the nitro button, the operator's keypad, the coin door),
//! emulated on a serial port (`WAL_NFS_FFB` set: the profile's force feedback strength).
//!
//! - **Port.** `SteeringManager` lists the ports (`SerialPort.GetPortNames`: the registry's
//!   `HARDWARE\DEVICEMAP\SERIALCOMM`, here a single [`PORT`]), skips COM1 and opens each
//!   (Mono's `SerialPort`, 115200 bauds: `kernel32`'s serial functions through `DllImport`,
//!   `payload-common`'s `serial`) until a board answers `GET_TARGET_INFO` with product 1, firmware
//!   5.35 (else it flashes the firmware). The LED strips and spotlight boards' managers then try
//!   the same port: refused while the steering board has it open, as a real port's exclusive
//!   access (else their probes would read its stream).
//! - **Frames.** `FE src cmd len(u16) payload.. sum`: `sum` makes the xor of `FE` and every
//!   byte after it 0; the board escapes a `FE` byte as `FE FE` (the PC escapes only its sum).
//! - **Board to PC.** Every poll (15 ms) `EVENT_STEERING_POSITION` (14: wheel -1..=1, positive
//!   right; gas and brake 0..=1; buttons: 1 nitro, 2 brake), on a change `EVENT_BUTTON` (10:
//!   card 0, keys: 1 operator, 2 down, 4 select, 8 up) and `EVENT_COIN` (11: player 0, the coin
//!   counter: the game credits its increase). `GET_INIT_STATUS` (30): no error.
//! - **Force feedback.** The board's firmware makes the forces from the game's `GET_CAR_INFO`
//!   (19, every physics step of a race: speed 0..=1, hit props, side hit -50..=50, vibration
//!   frequency and intensity, off road, vibration on) and `SET_FORCE_FACTOR` (20), its motor on
//!   between `ENABLE_WHEEL` (16) and `DISABLE_WHEEL` (17). Played on player 1's wheel
//!   (`output::FFB_*`): a centring spring growing with the speed, the side hits as a constant
//!   force (held, fading in 120 ms), the drift / off road vibration and the props' and landings'
//!   hits as a vibration; `WAL_NFS_FFB` scales them (percent, 0 off).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use wal_payload_common::{log, serial};
use wal_protocol::Axis;
use wal_protocol::output::{FFB_CONSTANT, FFB_MAX, FFB_SPRING, FFB_VIBRATION};

use crate::{Native, held};

/// The board's port, the only one listed.
const PORT: &str = "COM3";
const DLE: u8 = 0xFE;

const GET_TARGET_INFO: u8 = 1;
const EVENT_BUTTON: u8 = 10;
const EVENT_COIN: u8 = 11;
const EVENT_STEERING_POSITION: u8 = 14;
const ENABLE_WHEEL: u8 = 16;
const DISABLE_WHEEL: u8 = 17;
const GET_CAR_INFO: u8 = 19;
const SET_FORCE_FACTOR: u8 = 20;
const GET_ERROR: u8 = 30;

/// Firmware 5.35, hardware 1.0, product 1.
const TARGET_INFO: [u8; 6] = [5, 35, 1, 0, 1, 0];

const KEY_OPERATOR: u16 = 1;
const KEY_DOWN: u16 = 2;
const KEY_SELECT: u16 = 4;
const KEY_UP: u16 = 8;

/// `WAL_NFS_FFB` set: the game is Need for Speed Heat Takedown.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_NFS_FFB").is_ok())
}

/// `WAL_NFS_FFB`: the force feedback's strength, percent.
fn ffb_strength() -> i32 {
    static STRENGTH: OnceLock<i32> = OnceLock::new();
    *STRENGTH.get_or_init(|| std::env::var("WAL_NFS_FFB").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(100).clamp(0, 200))
}

/// A board frame.
fn frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = vec![0, cmd, payload.len() as u8, (payload.len() >> 8) as u8];
    body.extend_from_slice(payload);
    let sum = body.iter().fold(DLE, |s, b| s ^ b);
    body.push(sum);
    let mut out = vec![DLE];
    for b in body {
        out.push(b);
        if b == DLE {
            out.push(DLE);
        }
    }
    out
}

/// The game's last car info and force factor, the motor's state.
struct Car {
    enabled: bool,
    force_factor: f32,
    speed: f32,
    side: f32,
    vibration: f32,
    /// A hit's vibration and when.
    hit: (f32, Instant),
    /// The held side force and when.
    held_side: (f32, Instant),
}

static CAR: Mutex<Option<Car>> = Mutex::new(None);

fn car<R>(f: impl FnOnce(&mut Car) -> R) -> R {
    let mut car = CAR.lock().unwrap();
    let car = car.get_or_insert_with(|| Car {
        enabled: false,
        force_factor: 1.0,
        speed: 0.0,
        side: 0.0,
        vibration: 0.0,
        hit: (0.0, Instant::now()),
        held_side: (0.0, Instant::now()),
    });
    f(car)
}

fn f32_at(p: &[u8], i: usize) -> f32 {
    p.get(i..i + 4).map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap()))
}

/// A PC command: `FE 00 cmd len(u16) payload.. sum`.
fn handler(packet: &[u8]) -> Vec<u8> {
    if packet.len() < 6 || packet[0] != DLE {
        return Vec::new();
    }
    let cmd = packet[2];
    let len = usize::from(u16::from_le_bytes([packet[3], packet[4]]));
    let payload = &packet[5..(5 + len).min(packet.len())];
    match cmd {
        GET_TARGET_INFO => frame(cmd, &TARGET_INFO),
        GET_ERROR => frame(cmd, &[0, 0]),
        ENABLE_WHEEL | DISABLE_WHEEL => {
            car(|c| c.enabled = cmd == ENABLE_WHEEL);
            log!("steering: wheel {}", if cmd == ENABLE_WHEEL { "enabled" } else { "disabled" });
            frame(cmd, &[])
        }
        GET_CAR_INFO => {
            let (speed, props, side, intensity) = (f32_at(payload, 0), f32_at(payload, 4), f32_at(payload, 8), f32_at(payload, 16));
            let vibration_on = payload.get(22..24).is_some_and(|b| b != [0, 0]);
            car(|c| {
                let now = Instant::now();
                c.speed = speed.clamp(0.0, 1.0);
                c.side = (side / 20.0).clamp(-1.0, 1.0);
                if c.side.abs() >= held_side(c, now).abs() {
                    c.held_side = (c.side, now);
                }
                c.vibration = if vibration_on { (intensity * 4.0).clamp(0.0, 1.0) } else { 0.0 };
                if props != 0.0 {
                    c.hit = ((props.abs() / 11.0).clamp(0.0, 1.0), now);
                }
            });
            Vec::new()
        }
        SET_FORCE_FACTOR => {
            car(|c| c.force_factor = f32_at(payload, 0).clamp(0.0, 1.0));
            Vec::new()
        }
        // the others (debug variables, system events, the nitro button's light): acknowledged
        _ => frame(cmd, &[]),
    }
}

/// The held side force, fading in 120 ms.
fn held_side(c: &Car, now: Instant) -> f32 {
    let fade = 1.0 - now.duration_since(c.held_side.1).as_secs_f32() / 0.12;
    c.held_side.0 * fade.max(0.0)
}

/// The wheel's forces (spring, constant, vibration).
fn forces() -> (i32, i32, i32) {
    let full = (FFB_MAX * ffb_strength() / 100) as f32;
    car(|c| {
        if !c.enabled {
            return (0, 0, 0);
        }
        let now = Instant::now();
        let spring = (0.25 + 0.5 * c.speed) * c.force_factor;
        let constant = if held_side(c, now).abs() > c.side.abs() { held_side(c, now) } else { c.side };
        let hit = c.hit.0 * (1.0 - now.duration_since(c.hit.1).as_secs_f32() / 0.25).max(0.0);
        let vibration = c.vibration.max(hit);
        ((spring * full) as i32, (constant.clamp(-1.0, 1.0) * full) as i32, (vibration * full) as i32)
    })
}

static KEYS: AtomicU16 = AtomicU16::new(0);
static COINS: AtomicU16 = AtomicU16::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);

/// The board's events: the wheel and pedals every poll, the keys and the coin counter on a change.
fn idle() -> Vec<u8> {
    let stick = wal_payload_common::input(0);
    let wheel = (f32::from(stick.axis(Axis::LeftX)) / 32767.0).clamp(-1.0, 1.0);
    let pedal = |a: Axis| (f32::from(stick.axis(a).max(0)) / 32767.0).clamp(0.0, 1.0);
    let gas = if held(0, Native::Gas) { 1.0 } else { pedal(Axis::Accel) };
    let brake = if held(0, Native::Brake) { 1.0 } else { pedal(Axis::Brake) };
    let buttons = u8::from(held(0, Native::Nitro)) | (u8::from(brake > 0.5) << 1);
    let mut payload = Vec::with_capacity(13);
    for v in [wheel, gas, brake] {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    payload.push(buttons);
    let mut out = frame(EVENT_STEERING_POSITION, &payload);

    let keys = [(Native::Operator, KEY_OPERATOR), (Native::Down, KEY_DOWN), (Native::Select, KEY_SELECT), (Native::Up, KEY_UP)]
        .iter()
        .filter(|(n, _)| held(0, *n))
        .fold(0, |k, (_, bit)| k | bit);
    if KEYS.swap(keys, Ordering::Relaxed) != keys {
        out.extend(frame(EVENT_BUTTON, &[0, 0, keys as u8, (keys >> 8) as u8]));
    }
    let coin = (0..wal_protocol::MAX_PLAYERS).any(|p| held(p, Native::Coin));
    if coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        let count = COINS.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        out.extend(frame(EVENT_COIN, &[0, 0, count as u8, (count >> 8) as u8]));
    } else if !coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    out
}

/// Plays the wheel's forces on player 1's wheel, on change, every 10 ms.
fn ffb_thread() {
    let mut sent = (i32::MIN, i32::MIN, i32::MIN);
    let (mut logged, mut lines) = (Instant::now(), 0);
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let (spring, constant, vibration) = forces();
        // a trace of the forces: at most a line a second, the first 60
        if (spring, constant, vibration) != sent && lines < 60 && logged.elapsed() >= Duration::from_secs(1) {
            log!("steering: ffb spring {spring}, constant {constant}, vibration {vibration}");
            (logged, lines) = (Instant::now(), lines + 1);
        }
        if sent.0 != spring {
            wal_payload_common::send_output(0, FFB_SPRING, spring);
        }
        if sent.1 != constant {
            wal_payload_common::send_output(0, FFB_CONSTANT, constant);
        }
        if sent.2 != vibration {
            wal_payload_common::send_output(0, FFB_VIBRATION, vibration);
        }
        sent = (spring, constant, vibration);
    }
}

pub fn init() {
    // the originals of the serial functions (no module's imports to hook: Mono's `DllImport`s)
    serial::install_streaming_in(PORT, handler, idle, &[]);
    if ffb_strength() > 0 {
        std::thread::spawn(ffb_thread);
    }
    log!("steering: Need for Speed steering board on {PORT}, force feedback {}%", ffb_strength());
}

// The port, exclusive: refused while open.

type P = *mut c_void;
static OPEN: AtomicBool = AtomicBool::new(false);

fn is_board(h: P) -> bool {
    serial::port_of(h).is_some_and(|p| p.eq_ignore_ascii_case(PORT))
}

/// A second open of the board's port refused (the LED strips and spotlight managers try it
/// again every few seconds: logged once).
fn refused(name: &str) -> bool {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    let refuse = OPEN.load(Ordering::Relaxed) && name.strip_prefix("\\\\.\\").unwrap_or(name).eq_ignore_ascii_case(PORT);
    if refuse {
        if !LOGGED.swap(true, Ordering::Relaxed) {
            log!("steering: {PORT} already open: access denied");
        }
        unsafe { windows_sys::Win32::Foundation::SetLastError(5) }; // ERROR_ACCESS_DENIED
    }
    refuse
}

fn opened(h: P) -> P {
    if is_board(h) {
        OPEN.store(true, Ordering::Relaxed);
    }
    h
}

/// `WinSerialStream`'s `CreateFile` (ANSI).
unsafe extern "system" fn create_file_a(name: *const u8, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    type F = unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P;
    if !name.is_null() && refused(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy()) {
        return usize::MAX as P;
    }
    let create: F = unsafe { std::mem::transmute(serial::replacement("CreateFileA").unwrap()) };
    opened(unsafe { create(name, a, s, sa, d, f, t) })
}

unsafe extern "system" fn create_file_w(name: *const u16, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    type F = unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P;
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        if refused(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) })) {
            return usize::MAX as P;
        }
    }
    let create: F = unsafe { std::mem::transmute(serial::replacement("CreateFileW").unwrap()) };
    opened(unsafe { create(name, a, s, sa, d, f, t) })
}

unsafe extern "system" fn close_handle(h: P) -> i32 {
    type F = unsafe extern "system" fn(P) -> i32;
    if is_board(h) {
        OPEN.store(false, Ordering::Relaxed);
    }
    let close: F = unsafe { std::mem::transmute(serial::replacement("CloseHandle").unwrap()) };
    unsafe { close(h) }
}

/// The board's port's `kernel32` functions.
pub fn resolve_kernel32(name: &str) -> Option<usize> {
    match name {
        "CreateFile" | "CreateFileA" => Some(create_file_a as *const () as usize),
        "CreateFileW" => Some(create_file_w as *const () as usize),
        "CloseHandle" => Some(close_handle as *const () as usize),
        _ => serial::replacement(name),
    }
}

// The serial port list (`HARDWARE\DEVICEMAP\SERIALCOMM`): the board's port only.

/// The list's key handle, never a real one.
pub const PORTS_KEY: usize = 0x5e71a1;
const VALUE_NAME: &str = "\\Device\\WalSteering";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// `RegQueryInfoKeyW`: one value, no sub key.
unsafe extern "system" fn reg_query_info_key_w(key: P, class: *mut u16, class_len: *mut u32, reserved: *mut u32, sub_keys: *mut u32, max_sub_key: *mut u32, max_class: *mut u32, values: *mut u32, max_value_name: *mut u32, max_value: *mut u32, security: *mut u32, last_write: *mut u64) -> i32 {
    type F = unsafe extern "system" fn(P, *mut u16, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u64) -> i32;
    if key as usize != PORTS_KEY {
        return unsafe { advapi32::<F>(c"RegQueryInfoKeyW")(key, class, class_len, reserved, sub_keys, max_sub_key, max_class, values, max_value_name, max_value, security, last_write) };
    }
    let set = |p: *mut u32, v: u32| {
        if !p.is_null() {
            unsafe { *p = v };
        }
    };
    set(class_len, 0);
    set(sub_keys, 0);
    set(max_sub_key, 0);
    set(max_class, 0);
    set(values, 1);
    set(max_value_name, VALUE_NAME.len() as u32);
    set(max_value, (wide(PORT).len() * 2) as u32);
    set(security, 0);
    if !last_write.is_null() {
        unsafe { *last_write = 0 };
    }
    0
}

/// `RegEnumValueW`: the board's value.
unsafe extern "system" fn reg_enum_value_w(key: P, index: u32, name: *mut u16, name_len: *mut u32, reserved: *mut u32, kind: *mut u32, data: *mut u8, data_len: *mut u32) -> i32 {
    type F = unsafe extern "system" fn(P, u32, *mut u16, *mut u32, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
    if key as usize != PORTS_KEY {
        return unsafe { advapi32::<F>(c"RegEnumValueW")(key, index, name, name_len, reserved, kind, data, data_len) };
    }
    if index > 0 {
        return 259; // ERROR_NO_MORE_ITEMS
    }
    let value = wide(VALUE_NAME);
    if name_len.is_null() || (unsafe { *name_len } as usize) < value.len() {
        return 234; // ERROR_MORE_DATA
    }
    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), name, value.len()) };
    unsafe { *name_len = (value.len() - 1) as u32 };
    unsafe { query_value(kind, data, data_len) }
}

/// `RegQueryValueExW`: the port's name (REG_SZ).
unsafe extern "system" fn reg_query_value_ex_w(key: P, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, data_len: *mut u32) -> i32 {
    type F = unsafe extern "system" fn(P, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
    if key as usize != PORTS_KEY {
        return unsafe { advapi32::<F>(c"RegQueryValueExW")(key, name, reserved, kind, data, data_len) };
    }
    unsafe { query_value(kind, data, data_len) }
}

unsafe fn query_value(kind: *mut u32, data: *mut u8, data_len: *mut u32) -> i32 {
    let port = wide(PORT);
    let size = (port.len() * 2) as u32;
    if !kind.is_null() {
        unsafe { *kind = 1 }; // REG_SZ
    }
    if data_len.is_null() {
        return 0;
    }
    let room = unsafe { *data_len };
    unsafe { *data_len = size };
    if data.is_null() {
        return 0;
    }
    if room < size {
        return 234; // ERROR_MORE_DATA
    }
    unsafe { std::ptr::copy_nonoverlapping(port.as_ptr().cast::<u8>(), data, size as usize) };
    0
}

unsafe extern "system" fn reg_close_key(key: P) -> i32 {
    type F = unsafe extern "system" fn(P) -> i32;
    if key as usize == PORTS_KEY {
        return 0;
    }
    unsafe { advapi32::<F>(c"RegCloseKey")(key) }
}

/// A function of the real `advapi32.dll`.
unsafe fn advapi32<F: Copy>(name: &std::ffi::CStr) -> F {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let module = unsafe { LoadLibraryW(windows_sys::w!("advapi32.dll")) };
    let f = unsafe { GetProcAddress(module, name.as_ptr().cast()) }.map_or(0, |f| f as usize);
    unsafe { std::mem::transmute_copy(&f) }
}

/// The port list's `advapi32` functions (`RegOpenKeyExW` is the crate's).
pub fn resolve_advapi32(name: &str) -> Option<usize> {
    match name.strip_suffix('W').unwrap_or(name) {
        "RegQueryInfoKey" => Some(reg_query_info_key_w as *const () as usize),
        "RegEnumValue" => Some(reg_enum_value_w as *const () as usize),
        "RegQueryValueEx" => Some(reg_query_value_ex_w as *const () as usize),
        "RegCloseKey" => Some(reg_close_key as *const () as usize),
        _ => None,
    }
}
