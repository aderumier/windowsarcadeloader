//! Radikal Bikers (Gaelco 1998, the community Windows emulator of its Gaelco 3D board:
//! `radikalb.exe` 0.9, a 68EC020 core, Direct3D 8, the ROMs in `radikalb.zip`, 32-bit):
//! `WAL_ARCADEPC_BOARD=radikalbikers`.
//!
//! **Setup dialog.** Before the game the emulator shows a dialog (resource 101: controller
//! keyboard / mouse, windowed, full-screen mode); its OK fills the settings (0xb5ca5a0, the
//! `radikalb.dat` block: version, controller, mode, windowed, then the board's 512-byte
//! EEPROM), Cancel quits. The payload answers its `DialogBoxParamA` with OK without showing it:
//! windowed (0x48f004, the full-screen flag, 0) at the centered 4:3 rectangle of the screen
//! (0x483040 / 0x483044, the back buffer and the window's client size; 576x432 by default, the
//! board's picture), the window the profile's popup scaled to that rectangle.
//!
//! **Inputs.** Each frame 0x4027e0 builds the board's input bytes from `GetAsyncKeyState`
//! (active low): 0x6a8820d player 1 (0x08 handlebar up, 0x10 view, 0x20 brake, 0x40
//! accelerator, 0x80 start), 0x6a8821d player 2 (none on this cabinet), 0x6a8822c the system
//! (0x01 coin, 0x04 service coin, 0x08 test; 0x10 kept), and the handlebar's ADC (0x483148,
//! 0x10..0xf0, 0x80 centered) from the arrows (5 per frame) or the mouse. The payload replaces
//! it (a `jmp` to [`read_inputs`]): the same bytes from the virtual sticks, the handlebar the
//! left stick (the steering buttons fully turned). The real keyboard and mouse are filtered.
//!
//! **Free play** (the common `WAL_FREEPLAY`, on by default): the game has none (its GAME
//! ASSIGNMENTS take 1 to 8 coins a credit); a start press out of a game without credits (in
//! the 68020's work RAM: the game state byte 0x755c53b, 0 in the attract, its demo race
//! included, 2 from a few seconds after the start to GAME OVER; the credits word 0x755eb0c)
//! inserts a coin first (held 6 frames, released 6), then presses start; no other coin until
//! that game is over (the circuit select's confirm is a start press too). A continue still
//! takes a coin. Off: a coin per credit.
//!
//! **Saves.** The emulator writes `radikalb.dat` (its settings and the EEPROM: the operator
//! settings, the records) only when it quits on `WM_QUIT` (0x401420); the launcher stops it
//! with the process. The payload calls that save each time the block changes.
//!
//! Natives: `accelerator`, `brake`, `lift` (handlebar up), `view`, `start`, `coin`,
//! `service` (a service coin), `test`, `steer_left`, `steer_right`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

/// The emulator's input reader, its first bytes (`mov al, 0xff; sub esp, 0x18`).
const READ_INPUTS: usize = 0x4027e0;
const READ_INPUTS_CODE: [u8; 5] = [0xB0, 0xFF, 0x83, 0xEC, 0x18];
/// The board's input bytes (active low) and the handlebar's ADC.
const PLAYER1: usize = 0x6a8820d;
const PLAYER2: usize = 0x6a8821d;
const SYSTEM: usize = 0x6a8822c;
const HANDLEBAR: usize = 0x483148;
/// The back buffer's size, the full-screen flag.
const WIDTH: usize = 0x483040;
const HEIGHT: usize = 0x483044;
const FULL_SCREEN: usize = 0x48f004;
/// `radikalb.dat`'s block and its writer.
const SETTINGS: usize = 0xb5ca5a0;
const SETTINGS_LEN: usize = 0x218;
const SAVE: usize = 0x401420;
/// The game's credits (a 16-bit word of the emulated work RAM, little-endian) and its state
/// (0 out of a game).
const CREDITS: usize = 0x755eb0c;
const GAME_STATE: usize = 0x755c53b;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Accelerator,
    Brake,
    Lift,
    View,
    Start,
    Coin,
    Service,
    Test,
    SteerLeft,
    SteerRight,
}

/// The (byte, bit) of a board input.
fn input_bit(n: Native) -> Option<(usize, u8)> {
    Some(match n {
        Native::Lift => (PLAYER1, 0x08),
        Native::View => (PLAYER1, 0x10),
        Native::Brake => (PLAYER1, 0x20),
        Native::Accelerator => (PLAYER1, 0x40),
        Native::Start => (PLAYER1, 0x80),
        Native::Coin => (SYSTEM, 0x01),
        Native::Service => (SYSTEM, 0x04),
        Native::Test => (SYSTEM, 0x08),
        Native::SteerLeft | Native::SteerRight => return None,
    })
}

const NATIVES: &[(&str, Native)] = &[
    ("accelerator", Native::Accelerator),
    ("brake", Native::Brake),
    ("lift", Native::Lift),
    ("view", Native::View),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("service", Native::Service),
    ("test", Native::Test),
    ("steer_left", Native::SteerLeft),
    ("steer_right", Native::SteerRight),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "accelerator"),
    ("b2", "brake"),
    ("b3", "lift"),
    ("b4", "view"),
    ("up", "accelerator"),
    ("down", "brake"),
    ("left", "steer_left"),
    ("right", "steer_right"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

static FREE_PLAY: AtomicBool = AtomicBool::new(false);
/// Free play: the frames since its last coin (`u32::MAX`: none pending), whether the game has
/// started since (its state 2), and whether start was held the frame before.
static COIN_FRAME: AtomicU32 = AtomicU32::new(u32::MAX);
static STARTED: AtomicBool = AtomicBool::new(false);
static START_WAS: AtomicBool = AtomicBool::new(false);
/// The coin's frames, its release's, before the start goes through.
const COIN_FRAMES: u32 = 6;
/// A coin no game followed: free play again after 20 s.
const COIN_TIMEOUT: u32 = 20 * 60;

/// Free play: a coin before a start press out of a game without credits, then the start. The
/// game's state turns 2 a few seconds after the start (in the circuit select): no other coin
/// until its game is over (2, then 0 again).
fn free_play(start: bool, player1: &mut u8, system: &mut u8) {
    let (credits, state) = unsafe { (*(exe(CREDITS) as *const u16), *(exe(GAME_STATE) as *const u8)) };
    let mut frame = COIN_FRAME.load(Ordering::Relaxed);
    if frame != u32::MAX {
        if state == 2 {
            STARTED.store(true, Ordering::Relaxed);
        }
        if (STARTED.load(Ordering::Relaxed) && state == 0) || (!STARTED.load(Ordering::Relaxed) && frame > COIN_TIMEOUT) {
            frame = u32::MAX;
        }
    }
    if frame == u32::MAX && start && !START_WAS.load(Ordering::Relaxed) && credits == 0 && state == 0 {
        frame = 0;
        STARTED.store(false, Ordering::Relaxed);
    }
    if frame < COIN_FRAMES {
        *system &= !0x01;
    }
    if frame < 2 * COIN_FRAMES {
        *player1 |= 0x80;
    } else if frame < 3 * COIN_FRAMES {
        // the start, held for the coin's time once released
        *player1 &= !0x80;
    }
    COIN_FRAME.store(if frame == u32::MAX { frame } else { frame + 1 }, Ordering::Relaxed);
}

fn exe(va: usize) -> usize {
    unsafe { GetModuleHandleW(std::ptr::null()) as usize + va - 0x400000 }
}

/// The emulator's input reader replaced: the board's bytes from player 1's virtual stick.
extern "C" fn read_inputs() {
    let stick = input(0);
    let (mut player1, mut system) = (0xFFu8, 0xFFu8);
    let (mut left, mut right) = (false, false);
    map().for_each_pressed(&stick, |n| match n {
        Native::SteerLeft => left = true,
        Native::SteerRight => right = true,
        n => match input_bit(n) {
            Some((PLAYER1, bit)) => player1 &= !bit,
            Some((_, bit)) => system &= !bit,
            None => {}
        },
    });
    // the pads' triggers past a quarter
    if stick.axis(Axis::Accel) > 8192 {
        player1 &= !0x40;
    }
    if stick.axis(Axis::Brake) > 8192 {
        player1 &= !0x20;
    }
    let start = player1 & 0x80 == 0;
    if FREE_PLAY.load(Ordering::Relaxed) {
        free_play(start, &mut player1, &mut system);
    }
    START_WAS.store(start, Ordering::Relaxed);
    let lx = match (left, right) {
        (true, false) => -32767,
        (false, true) => 32767,
        _ => stick.axis(Axis::LeftX) as i32,
    };
    unsafe {
        *(exe(PLAYER1) as *mut u8) = player1;
        *(exe(PLAYER2) as *mut u8) = 0xFF;
        // bit 0x10 is not an input: kept
        let sys = exe(SYSTEM) as *mut u8;
        *sys = system & !0x10 | *sys & 0x10;
        *(exe(HANDLEBAR) as *mut i32) = (0x80 + lx * 0x70 / 32767).clamp(0x10, 0xF0);
    }
}

/// The setup dialog: OK, not shown.
unsafe extern "system" fn dialog_box_param(_inst: *mut u8, _template: *const u8, _parent: *mut u8, _proc: usize, _param: isize) -> isize {
    1
}

/// Saves `radikalb.dat` (the emulator's own writer) each time its block changes.
fn save_thread() {
    let block = || unsafe { std::slice::from_raw_parts(exe(SETTINGS) as *const u8, SETTINGS_LEN).to_vec() };
    let save: extern "C" fn() = unsafe { std::mem::transmute(exe(SAVE)) };
    // the block as read from the file, its version word set by the writer
    std::thread::sleep(Duration::from_secs(10));
    let mut saved = block();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = block();
        if now[4..] != saved[4..] {
            save();
            log!("radikalbikers: radikalb.dat saved");
            saved = now;
        }
    }
}

pub fn init() {
    let code = unsafe { std::slice::from_raw_parts(exe(READ_INPUTS) as *const u8, READ_INPUTS_CODE.len()) };
    if code != READ_INPUTS_CODE {
        log!("radikalbikers: unknown radikalb.exe, nothing patched");
        return;
    }
    let exe_module = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe_module, |_| false);
    wal_payload_common::input_filter::own_mouse(exe_module);
    unsafe { wal_payload_common::iat::hook("user32.dll", "DialogBoxParamA", dialog_box_param as *const () as usize) };
    // windowed, the centered 4:3 rectangle of the screen
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let (w, h) = if sw * 3 > sh * 4 { (sh * 4 / 3, sh) } else { (sw, sw * 3 / 4) };
    unsafe {
        *(exe(FULL_SCREEN) as *mut u32) = 0;
        *(exe(WIDTH) as *mut i32) = w;
        *(exe(HEIGHT) as *mut i32) = h;
    }
    let at = exe(READ_INPUTS);
    let mut jump = [0xE9u8, 0, 0, 0, 0];
    jump[1..].copy_from_slice(&(read_inputs as *const () as usize).wrapping_sub(at + 5).to_le_bytes());
    unsafe {
        let mut old = 0;
        VirtualProtect(at as *const _, jump.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(jump.as_ptr(), at as *mut u8, jump.len());
        VirtualProtect(at as *const _, jump.len(), old, &mut old);
    }
    let free = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(false);
    FREE_PLAY.store(free, Ordering::Relaxed);
    std::thread::spawn(save_thread);
    log!("radikalbikers: setup dialog skipped, windowed {w}x{h}, inputs from the virtual sticks, free play {free}");
}
