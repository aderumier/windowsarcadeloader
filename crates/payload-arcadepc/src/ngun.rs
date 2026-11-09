//! Akuma Mortis Immortal (`game.exe`, window class `NGUN`, Direct3D 9, 32-bit):
//! `WAL_ARCADEPC_BOARD=ngun`, x86 build.
//!
//! - **Board.** The cabinet's gun board is on the parallel port, through `inpout32.dll`
//!   (`LoadLibraryA("inpout32")`, `Inp32` / `Out32`, stdcall): the payload answers both (the
//!   dump's library, a driver installer, is never loaded). A command is the data port's byte
//!   (0x378) strobed by the control port (0x37a: 0xd0, 0xd1, 0xd0); its answer is read back from
//!   the data port in input mode (control 0xf8, 0xf9). Commands: 1 / 2 player 1's gun x / y,
//!   3 / 4 player 2's, 5 the guns' bits (bit 0 player 1's trigger, 2 its gun on the screen; 4, 6
//!   player 2's), 6 the triggers alone, 7 / 8 the switches (bit 0 player 1 start, 1 player 2
//!   start, 2 test, 3 service, 4 coin; active high). Bytes `x << 4 | 0xd`, `| 0xe` are the lamps
//!   and recoil (ignored). The status port (0x379): bits 1-2 the board is there (else no reader
//!   thread), bit 7 a gun sample is ready (the reader then asks 1..5), bit 5 picks command 7 or 8.
//!   Each sample makes the game draw one flash frame (the screen filled with the settings'
//!   color, cyan, for the gun's sensor): a sample is asked at each trigger (or reload) press;
//!   the game reads the triggers alone (command 6) at each frame, releases included.
//! - **Aim.** The game maps a gun's x / y bytes through its calibration (`0x590d24` + 0x20 per
//!   player: x min, x max, y min, y max, floats: the calibrated center byte -100 / +100,
//!   -115 / +115): x = (byte - min) * 3.1875, y = (byte - min) * 2.0788 over 640x480. The payload
//!   sends the byte of the gun's place in the picture through the current calibration (the game's
//!   gun calibration then keeps the center). A trigger off the screen (or `reload`) is a shot with
//!   the screen bit clear (the game's reload).
//! - **Settings.** `expdat` / `extdat` (each byte negated, check sums): the boot's settings
//!   reader fills `0x590f10`..; bit 0 of `0x590f2c` is free play, copied by `0x418de0` (the boot
//!   calls it at `0x4074ab`): the common `WAL_FREEPLAY` sets the bit before that copy; unset,
//!   the file's.
//! - **Protection.** A Dinkey dongle (`DK2win32.dll`): the profile's patches.
//!
//! Natives: `trigger reload start coin` (each player), `test service`.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log, screenshot};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

const PLAYERS: usize = 2;
const DATA: u16 = 0x378;
const STATUS: u16 = 0x379;
const CONTROL: u16 = 0x37A;
/// Status: the board is there (bits 1-2), a gun sample ready (bit 7).
const STATUS_PRESENT: u8 = 0x06;
const STATUS_SAMPLE: u8 = 0x80;
/// A coin press stays on the wire this long, whatever the game's polling.
const COIN_HOLD: Duration = Duration::from_millis(150);

/// Player 1's calibration (x min, x max, y min, y max), relative to the executable; player 2's
/// 0x20 further.
const CALIBRATION: usize = 0x190D24;
/// The boot's call of the settings copy (`call 0x418de0`), the copy, the settings' flags.
const SETTINGS_CALL: usize = 0x74AB;
const SETTINGS_CALL_CODE: [u8; 5] = [0xE8, 0x30, 0x19, 0x01, 0x00];
const SETTINGS_COPY: usize = 0x18DE0;
const FLAGS: usize = 0x190F2C;
/// The calibrated span of a gun's bytes over the picture (640 / 3.1875, 480 / 2.0788).
const SPAN_X: f32 = 640.0 / (1.275 * 2.5);
const SPAN_Y: f32 = 480.0 / (1.108_695_6 * 1.875);

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Start,
    Coin,
    /// A bit of the switches byte (player 1's only).
    Switch(u8),
    /// The gun points off the screen.
    Offscreen,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("reload", Native::Reload),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("test", Native::Switch(2)),
    ("service", Native::Switch(3)),
    ("offscreen", Native::Offscreen),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "reload"),
    ("b6", "offscreen"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "test"),
    ("service", "service"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

static EXE: AtomicUsize = AtomicUsize::new(0);

struct Board {
    /// The data port's last byte and the command strobed.
    data: u8,
    command: u8,
    control: u8,
    /// Each player's trigger held at the last status read, a sample asked.
    triggers: [bool; PLAYERS],
    last_coin: Option<Instant>,
}

static BOARD: Mutex<Board> = Mutex::new(Board { data: 0, command: 0, control: 0, triggers: [false; PLAYERS], last_coin: None });

/// `player`'s gun: its place in the picture (fractions), `None` off it.
fn gun(player: usize) -> Option<(f32, f32)> {
    let stick = input(player);
    let f = |v: i16| (v as f32 + 32768.0) / 65535.0;
    let [px, py, w, h] = screenshot::picture_rect();
    let (x, y) = ((f(stick.axis(Axis::LeftX)) - px) / w, (f(stick.axis(Axis::LeftY)) - py) / h);
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

/// The byte of `v` (a fraction of the picture) over a calibration from `min`, `span` bytes wide.
fn calibrated(min: f32, span: f32, v: f32) -> u8 {
    (min + span * v).round().clamp(0.0, 255.0) as u8
}

/// `player`'s gun x (`y` false) or y byte.
fn aim(player: usize, y: bool) -> u8 {
    let exe = EXE.load(Ordering::Relaxed);
    let (x, yv) = gun(player).unwrap_or((0.5, 0.5));
    // x min, y min of the player's calibration
    let at = exe + CALIBRATION + 0x20 * player + if y { 8 } else { 0 };
    let min = unsafe { *(at as *const f32) };
    if y { calibrated(min, SPAN_Y, yv) } else { calibrated(min, SPAN_X, x) }
}

/// Each player's trigger (or reload) held.
fn triggers() -> [bool; PLAYERS] {
    let mut held = [false; PLAYERS];
    for (player, h) in held.iter_mut().enumerate() {
        map().for_each_pressed(&input(player), |n| *h |= matches!(n, Native::Trigger | Native::Reload));
    }
    held
}

/// Command 5's bits: each player's trigger (bit 0 / 4) and gun on the screen (bit 2 / 6).
fn gun_bits() -> u8 {
    let mut bits = 0u8;
    for player in 0..PLAYERS {
        let (mut trigger, mut off) = (false, gun(player).is_none());
        map().for_each_pressed(&input(player), |n| match n {
            Native::Trigger => trigger = true,
            Native::Reload => (trigger, off) = (true, true),
            Native::Offscreen => off = true,
            _ => {}
        });
        bits |= (u8::from(trigger) | u8::from(!off) << 2) << (4 * player);
    }
    bits
}

/// The switches (commands 7 / 8).
fn switches(board: &mut Board) -> u8 {
    let mut bits = 0u8;
    for player in 0..PLAYERS {
        map().for_each_pressed(&input(player), |n| match n {
            Native::Start => bits |= 1 << player,
            Native::Coin => board.last_coin = Some(Instant::now()),
            Native::Switch(bit) if player == 0 => bits |= 1 << bit,
            _ => {}
        });
    }
    if board.last_coin.is_some_and(|t| t.elapsed() < COIN_HOLD) {
        bits |= 1 << 4;
    }
    bits
}

fn answer(board: &mut Board) -> u8 {
    match board.command {
        1 => aim(0, false),
        2 => aim(0, true),
        3 => aim(1, false),
        4 => aim(1, true),
        5 => gun_bits(),
        // the triggers alone (bits 0 / 4)
        6 => gun_bits() & 0x11,
        7 | 8 => switches(board),
        _ => 0,
    }
}

static READS: AtomicU8 = AtomicU8::new(0);

unsafe extern "system" fn inp32(port: u16) -> u16 {
    let mut board = BOARD.lock().unwrap();
    if READS.load(Ordering::Relaxed) == 0 {
        READS.store(1, Ordering::Relaxed);
        log!("ngun: first board read (port {port:#x})");
    }
    match port {
        DATA => u16::from(answer(&mut board)),
        STATUS => {
            // a sample at a trigger's press: bit 7 for one read
            let held = triggers();
            let due = (0..PLAYERS).any(|p| held[p] && !board.triggers[p]);
            board.triggers = held;
            u16::from(STATUS_PRESENT | if due { STATUS_SAMPLE } else { 0 })
        }
        CONTROL => u16::from(board.control),
        _ => 0xFF,
    }
}

unsafe extern "system" fn out32(port: u16, value: u16) {
    let value = value as u8;
    let mut board = BOARD.lock().unwrap();
    match port {
        DATA => board.data = value,
        CONTROL => {
            // the strobe's rising edge in output mode (bit 5 clear) takes the data byte
            if value & 0x21 == 0x01 && board.control & 0x01 == 0 {
                board.command = board.data;
            }
            board.control = value;
        }
        _ => {}
    }
}

type P = *mut std::ffi::c_void;

static LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);
static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);

/// `inpout32`: the payload (its reference counted, as the game's `FreeLibrary` at exit drops it).
unsafe extern "system" fn load_library(name: *const u8) -> P {
    use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleHandleExW};
    let f: unsafe extern "system" fn(*const u8) -> P = unsafe { std::mem::transmute(LOAD_LIBRARY.load(Ordering::Relaxed)) };
    if !name.is_null() {
        let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().to_ascii_lowercase();
        if n == "inpout32" || n == "inpout32.dll" {
            let mut module = std::ptr::null_mut();
            unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, inp32 as *const () as *const u16, &mut module) };
            log!("ngun: inpout32 is the payload");
            return module;
        }
    }
    unsafe { f(name) }
}

unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> usize {
    let f: unsafe extern "system" fn(P, *const u8) -> usize = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    // a name, not an ordinal
    if name as usize > 0xFFFF {
        match unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() {
            b"Inp32" => return inp32 as *const () as usize,
            b"Out32" => return out32 as *const () as usize,
            _ => {}
        }
    }
    unsafe { f(module, name) }
}

static FREE_PLAY: Mutex<Option<bool>> = Mutex::new(None);

/// The flags with the free play option (`None`: as they are).
fn with_free_play(flags: u32, free_play: Option<bool>) -> u32 {
    match free_play {
        Some(true) => flags | 1,
        Some(false) => flags & !1,
        None => flags,
    }
}

/// The boot's settings copy, after the free play option over the file's flag.
extern "C" fn copy_settings() {
    let exe = EXE.load(Ordering::Relaxed);
    let flags = (exe + FLAGS) as *mut u32;
    let free_play = *FREE_PLAY.lock().unwrap();
    unsafe { *flags = with_free_play(*flags, free_play) };
    if let Some(on) = free_play {
        log!("ngun: free play {}", if on { "on" } else { "off" });
    }
    let copy: extern "C" fn() = unsafe { std::mem::transmute(exe + SETTINGS_COPY) };
    copy();
}

/// Writes a `call` to `to` at `at`.
fn call(at: usize, to: usize) {
    let mut code = [0xE8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&to.wrapping_sub(at + 5).to_le_bytes());
    unsafe {
        let mut old = 0;
        VirtualProtect(at as *const _, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const _, code.len(), old, &mut old);
    }
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let settings = exe + SETTINGS_CALL;
    if unsafe { std::slice::from_raw_parts(settings as *const u8, 5) } != SETTINGS_CALL_CODE {
        log!("ngun: unknown game.exe build, no board");
        return;
    }
    EXE.store(exe, Ordering::Relaxed);
    *FREE_PLAY.lock().unwrap() = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY");
    call(settings, copy_settings as *const () as usize);
    unsafe {
        match iat::hook("KERNEL32.dll", "LoadLibraryA", load_library as *const () as usize) {
            Some(o) => LOAD_LIBRARY.store(o, Ordering::Relaxed),
            None => log!("ngun: no LoadLibraryA import"),
        }
        match iat::hook("KERNEL32.dll", "GetProcAddress", get_proc_address as *const () as usize) {
            Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
            None => log!("ngun: no GetProcAddress import"),
        }
    }
    log!("ngun: parallel port gun board");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aim_bytes() {
        // the dump's calibration: center 0x7f
        assert_eq!(calibrated(127.0 - 100.0, SPAN_X, 0.0), 27);
        assert_eq!(calibrated(127.0 - 100.0, SPAN_X, 0.5), 127);
        assert_eq!(calibrated(127.0 - 115.0, SPAN_Y, 0.5), 127);
        assert_eq!(calibrated(250.0, SPAN_X, 1.0), 255);
    }

    #[test]
    fn strobe() {
        *BOARD.lock().unwrap() = Board { data: 0, command: 0, control: 0xD0, triggers: [false; PLAYERS], last_coin: None };
        unsafe {
            out32(DATA, 7);
            out32(CONTROL, 0xD1);
            out32(CONTROL, 0xD0);
            // input mode: the strobe takes nothing
            out32(DATA, 3);
            out32(CONTROL, 0xF8);
            out32(CONTROL, 0xF9);
        }
        assert_eq!(BOARD.lock().unwrap().command, 7);
    }

    #[test]
    fn free_play() {
        assert_eq!(with_free_play(0x10, Some(true)), 0x11);
        assert_eq!(with_free_play(0x11, Some(false)), 0x10);
        assert_eq!(with_free_play(0x11, None), 0x11);
    }
}
