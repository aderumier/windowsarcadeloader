//! Friction (Venum Spyder, 2006, `Friction.exe`, Direct3D 9, 32-bit): `WAL_ARCADEPC_BOARD=vsioboard`,
//! x86 build.
//!
//! - **Board.** The game loads `vsIOBoard.dll` (`LoadLibraryA`, its functions by name, cdecl), the
//!   cabinet's gun board library: the payload is that library (`payload_install`, the dump's
//!   community replacement put aside). The game registers a listener (`vsIOBoard_AddListener`:
//!   a C++ object), whose methods (thiscall) the board calls from its thread, every 16 ms:
//!   `[0](event)` each gun's state (event `{2, player, 1, x, y, buttons}`: 32-bit, 32-bit, then
//!   16-bit words; x 160..480 and y 120..360 the screen's width and height, which the game maps
//!   with `vsIOBoard_Calibrate`; buttons 5 the trigger, 2 reload), `[1](mech)` a coin (1 player
//!   1's slot, 2 player 2's, 3 a service credit), `[2](1)` the service button, `[3](player)` a
//!   start button (1, 2). `vsIOBoard_SetUpGun(player, auto, _, frames)`: with `auto` 1, a held
//!   trigger is reported every `frames` events (the machine gun's rhythm).
//! - **Free play.** The game has none (coins per play and per continue, at least 1): with the
//!   common `WAL_FREEPLAY`, its coin counter (the coin object `[0x5450c4]`: coins `+0`, a play's
//!   price `+4`, at most 420) is kept at 99 plays, and the spend (`0x44ba47`: `sub` at
//!   `0x44ba64`) takes nothing: starts and continues free, the game showing 99 credits.
//! - **Start.** A trigger press is its player's start too (a shot starts, joins, continues: the
//!   listener's start, `[3](players)`, a bit per player, acts in the attract and the game only).
//! - **Game patches.** The community library also patched the game (its `RebootDisable` and
//!   `DInputDisable` options): the profile's `WAL_PATCHES` (the reboot call skipped, the game's
//!   own DirectInput mouse and keyboard reading skipped).
//!
//! Natives (each player): `trigger reload start coin`; `service test` (player 1's stick).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

const PLAYERS: usize = 2;
/// The gun event's type.
const GUN_EVENT: u32 = 2;
/// The event's buttons.
const TRIGGER: u16 = 5;
const RELOAD: u16 = 2;
/// The board's coordinates: x 160..480, y 120..360.
const X0: f32 = 160.0;
const Y0: f32 = 120.0;
const WIDTH: f32 = 320.0;
const HEIGHT: f32 = 240.0;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Start,
    Coin,
    Service,
    Test,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("reload", Native::Reload),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("service", Native::Service),
    ("test", Native::Test),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "reload"),
    ("b6", "reload"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The gun event the listener's first method takes.
#[repr(C)]
struct Event {
    kind: u32,
    player: u32,
    valid: u16,
    x: u16,
    y: u16,
    buttons: u16,
}

/// The listeners (C++ objects: their first word the method table).
static LISTENERS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);
/// `WAL_VSIOBOARD_TRACE=1`: the guns' events logged at each button change.
static TRACE: AtomicBool = AtomicBool::new(false);
/// Per player: the automatic fire (`vsIOBoard_SetUpGun`): on, frames between reports.
static AUTO: [AtomicBool; PLAYERS] = [const { AtomicBool::new(false) }; PLAYERS];
static AUTO_FRAMES: [AtomicU32; PLAYERS] = [const { AtomicU32::new(0) }; PLAYERS];

/// Calls method `index` of each listener with `arg` (thiscall).
fn notify(index: usize, arg: usize) {
    let listeners = LISTENERS.lock().unwrap().clone();
    for this in listeners {
        unsafe {
            let table = *(this as *const *const usize);
            let method: unsafe extern "thiscall" fn(usize, usize) = std::mem::transmute(*table.add(index));
            method(this, arg);
        }
    }
}

/// The game's coin object (`Friction.exe`'s pointer at `0x5450c4`; null before it exists).
fn coin_object() -> *mut u32 {
    unsafe { *(0x5450c4 as *const *mut u32) }
}

/// Free play: the counter kept at 99 plays (the spend patched out by [`init`]).
fn keep_credits() {
    let coins = coin_object();
    if coins.is_null() {
        return;
    }
    let price = unsafe { coins.add(1).read_volatile() }.clamp(1, 4);
    if unsafe { coins.read_volatile() } < 99 * price {
        unsafe { coins.write_volatile(99 * price) };
    }
}

/// The board's coordinates of `player`'s gun (its place in the game's picture; off it: the
/// nearest edge).
fn aim(player: usize) -> (u16, u16) {
    let (sx, sy) = gun::on_screen(&input(player), gun::screen_size());
    let (fx, fy) = match gun::client_area(gun::find_window()) {
        Some((left, top, w, h)) => ((sx - left) / w, (sy - top) / h),
        None => (0.5, 0.5),
    };
    ((X0 + fx.clamp(0.0, 1.0) * WIDTH).round() as u16, (Y0 + fy.clamp(0.0, 1.0) * HEIGHT).round() as u16)
}

/// The edge-triggered inputs held at the last poll: per player start, coin; service, test.
#[derive(Default)]
struct Held {
    start: [bool; PLAYERS],
    coin: [bool; PLAYERS],
    service: bool,
    test: bool,
}

/// A rising edge of `now` against `before` (updated).
fn pressed(before: &mut bool, now: bool) -> bool {
    let edge = now && !*before;
    *before = now;
    edge
}

fn board_thread() {
    let mut held = Held::default();
    let mut trigger_held = [false; PLAYERS];
    let mut auto_count = [0u32; PLAYERS];
    let mut last_buttons = [0u16; PLAYERS];
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let mut service = false;
        let mut test = false;
        if FREE_PLAY.load(Ordering::Relaxed) {
            keep_credits();
        }
        for player in 0..PLAYERS {
            let (mut trigger, mut reload, mut start, mut coin) = (false, false, false, false);
            map().for_each_pressed(&input(player), |n| match n {
                Native::Trigger => trigger = true,
                Native::Reload => reload = true,
                Native::Start => start = true,
                Native::Coin => coin = true,
                Native::Service => service |= player == 0,
                Native::Test => test |= player == 0,
            });
            let mut buttons = if trigger { TRIGGER } else { 0 } | if reload { RELOAD } else { 0 };
            // automatic fire: a held trigger reported once every `frames` events
            if trigger && AUTO[player].load(Ordering::Relaxed) {
                auto_count[player] += 1;
                if auto_count[player] >= AUTO_FRAMES[player].load(Ordering::Relaxed) {
                    auto_count[player] = 0;
                }
                if auto_count[player] != 0 {
                    buttons &= !TRIGGER;
                }
            } else {
                auto_count[player] = 0;
            }
            let (x, y) = aim(player);
            if TRACE.load(Ordering::Relaxed) && buttons != last_buttons[player] {
                log!("vsioboard: player {} x {x} y {y} buttons {buttons}", player + 1);
            }
            last_buttons[player] = buttons;
            let event = Event { kind: GUN_EVENT, player: player as u32, valid: 1, x, y, buttons };
            notify(0, &event as *const Event as usize);
            if pressed(&mut held.coin[player], coin) {
                notify(1, player + 1);
            }
            // a shot is the player's start too
            let shot = pressed(&mut trigger_held[player], trigger);
            if pressed(&mut held.start[player], start) || shot {
                notify(3, player + 1);
            }
        }
        if pressed(&mut held.service, service) {
            notify(1, 3);
        }
        if pressed(&mut held.test, test) {
            notify(2, 1);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_Init() -> i32 {
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(board_thread);
        log!("vsioboard: board started");
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_Close() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_AddListener(listener: *mut c_void) -> i32 {
    LISTENERS.lock().unwrap().push(listener as usize);
    log!("vsioboard: listener {listener:p}");
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_RemoveListener(listener: *mut c_void) -> i32 {
    LISTENERS.lock().unwrap().retain(|&l| l != listener as usize);
    1
}

/// The board's coordinates in a `width` x `height` picture (in place).
#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_Calibrate(_player: i32, width: i32, height: i32, x: *mut i32, y: *mut i32) -> i32 {
    unsafe {
        if !x.is_null() {
            *x = ((*x - X0 as i32) * width / WIDTH as i32).clamp(0, width);
        }
        if !y.is_null() {
            *y = ((*y - Y0 as i32) * height / HEIGHT as i32).clamp(0, height);
        }
        if y.is_null() { 0 } else { *y }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_SetUpGun(player: i32, auto: i32, _a: i32, frames: i32, _b: i32, _c: i32) -> i32 {
    if let Some(p) = usize::try_from(player).ok().filter(|&p| p < PLAYERS) {
        AUTO[p].store(auto == 1, Ordering::Relaxed);
        AUTO_FRAMES[p].store(frames.max(0) as u32, Ordering::Relaxed);
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_DisableGunFlashes() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_GetDeviceState(_a: i32, _b: i32, _c: i32, _d: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_GetGunParams(_a: i32, _b: *mut i32, _c: *mut i32, _d: *mut i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_GetNumCoins(_a: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_GetNumDefaultDevices() -> i32 {
    PLAYERS as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_SetCalibration(_a: i32, _b: i32, _c: i32, _d: i32, _e: i32, _f: i32, _g: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_SetFlashDelay(_a: i32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_SetGunAccuracy(_a: i32, _b: i32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_getHSyncPolarity() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_getVSyncPolarity() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn vsIOBoard_getVideoMode() -> i32 {
    1
}

pub fn init() {
    let free_play = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") == Some(true);
    FREE_PLAY.store(free_play, Ordering::Relaxed);
    TRACE.store(std::env::var("WAL_VSIOBOARD_TRACE").is_ok_and(|v| v.trim() == "1"), Ordering::Relaxed);
    if free_play {
        // the spend (`sub [esi], eax`): nothing taken
        let spend = 0x44ba64 as *mut [u8; 2];
        if unsafe { spend.read() } == [0x29, 0x06] {
            let mut old = 0;
            unsafe { VirtualProtect(spend.cast(), 2, PAGE_EXECUTE_READWRITE, &mut old) };
            unsafe { spend.write([0x90, 0x90]) };
            unsafe { VirtualProtect(spend.cast(), 2, old, &mut old) };
        } else {
            log!("vsioboard: the spend not this build's: free play off");
            FREE_PLAY.store(false, Ordering::Relaxed);
        }
    }
    log!("vsioboard: vsIOBoard.dll, free play {}", if FREE_PLAY.load(Ordering::Relaxed) { "on" } else { "off" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrate() {
        let (mut x, mut y) = (320, 240);
        vsIOBoard_Calibrate(0, 640, 480, &mut x, &mut y);
        assert_eq!((x, y), (320, 240));
        let (mut x, mut y) = (480, 120);
        vsIOBoard_Calibrate(0, 1280, 720, &mut x, &mut y);
        assert_eq!((x, y), (1280, 0));
    }
}
