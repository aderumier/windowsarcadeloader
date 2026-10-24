//! Action Deka (GDK, 2008): the payload is the game's `libacio.dll` (ACIO bus, card reader, the
//! GDKI action controllers, `gdki.rs`); deka.exe also imports `libbms.dll` (the BM-System
//! client, `bms.rs`), pointed at this DLL's functions of the same names.
//!
//! The cabinet's I/O board is a Cypress EZ-USB board (CyUSB driver: switches, coin counter,
//! lamps, security, the controllers' tactile transducers). deka.exe links Konami's EZ-USB
//! library: its flat `usb*` functions (cdecl, 0 done, 0x60 busy, -33 failed) are pointed at
//! the payload (a jump at each one's entry), the board answers at once.
//!
//! - Switches (`usbPadRead`, active high): TEST 0x8000, SERVICE 0x4000, P1 START 0x800,
//!   P2 START 0x400 (the facade's switches 0-3; switch 4 is 0x2000).
//! - Coins: the board's 16-bit coin counter, the game counts its steps.
//! - The tactile sound board (`data/tacsound/*.tac` uploaded, then played per controller): no
//!   device.
//!
//! The players' hands: on the cabinet a 3D sensor (an ultrasonic tracker on COM1, not started:
//! the profile's `-no3d`) and the controllers' accelerometers give each hand's position and
//! motions. Here the game's own debug input (DebugGameIO) gives them from each player's gun:
//! its per-frame update sets each controller's motion code (punch, guard, hold / throw,
//! special), its overrides answer the aim (the right hand, the left one not used) and the
//! trigger; the steering (the angle between the hands) comes from the d-pad. The boot's clock
//! and 3D sensor calibration checks are skipped; `WAL_KONAMI_UNLOCK_FINAL=1` offers the 4th
//! mission at once.
//!
//! Native inputs for `native_map`: `test service coin start trigger punch guard throw special
//! reload steer_left steer_right`.

use std::sync::Mutex;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Trigger,
    Punch,
    Guard,
    Throw,
    Special,
    Reload,
    SteerLeft,
    SteerRight,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("punch", Native::Punch),
    ("guard", Native::Guard),
    ("throw", Native::Throw),
    ("special", Native::Special),
    ("reload", Native::Reload),
    ("steer_left", Native::SteerLeft),
    ("steer_right", Native::SteerRight),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "trigger+punch"),
    ("b2", "guard+reload"),
    ("b3", "throw"),
    ("b4", "special"),
    ("left", "steer_left"),
    ("right", "steer_right"),
];

const PLAYERS: usize = 2;

const SW_TEST: u32 = 0x8000;
const SW_SERVICE: u32 = 0x4000;
const SW_START: [u32; PLAYERS] = [0x800, 0x400];

struct Board {
    maps: Vec<ButtonMap<Native>>,
    coin_held: bool,
    coins: u16,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

fn with_board<R>(f: impl FnOnce(&mut Board) -> R) -> R {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board {
        maps: (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        coin_held: false,
        coins: 0,
    });
    f(board)
}

const DONE: i32 = 0;

extern "C" fn usb_open(_mode: u32) -> i32 {
    log!("deka: EZ-USB board emulated");
    DONE
}

extern "C" fn usb_done() -> i32 {
    DONE
}

extern "C" fn usb_done1(_a: u32) -> i32 {
    DONE
}

extern "C" fn usb_done3(_a: u32, _b: u32, _c: u32) -> i32 {
    DONE
}

extern "C" fn usb_done4(_a: u32, _b: u32, _c: u32, _d: u32) -> i32 {
    DONE
}

/// The board was found / is alive: 1.
extern "C" fn usb_yes() -> i32 {
    1
}

extern "C" fn usb_nothing(_a: u32) {}

/// The switches (active high).
unsafe extern "C" fn usb_pad_read(out: *mut u32) -> i32 {
    let sticks: Vec<_> = (0..PLAYERS).map(wal_payload_common::input).collect();
    let bits = with_board(|b| {
        let mut bits = 0;
        let mut coin = false;
        for (player, stick) in sticks.iter().enumerate() {
            b.maps[player].for_each_pressed(stick, |n| match n {
                Native::Test => bits |= SW_TEST,
                Native::Service => bits |= SW_SERVICE,
                Native::Start => bits |= SW_START[player],
                Native::Coin => coin = true,
                _ => {}
            });
        }
        if coin && !b.coin_held {
            b.coins = b.coins.wrapping_add(1);
            log!("deka: coin");
        }
        b.coin_held = coin;
        bits
    });
    if !out.is_null() {
        unsafe { *out = bits };
    }
    DONE
}

/// The coin counter of slot `_slot`.
extern "C" fn usb_coin(_slot: u32) -> i32 {
    i32::from(with_board(|b| b.coins))
}

// The controllers, through the game's own debug input (DebugGameIO, `*0x713084`): its per-frame
// update sets each controller's motion code (+0x2c, the previous frame's at +0x3c, enabled by
// +0x50), and its overrides answer the controller API (aim, trigger) before the sensors.

/// The motion codes (the game's motion names).
const GUARD: i32 = 0x10;
const ST_UP_HARD: i32 = 0x12;
const ST_DOWN_HARD: i32 = 0x14;
const HOLD: i32 = 0x19;
const THROW: i32 = 0x1b;
const SPECIAL: i32 = 0x1c;

/// A player's controls of the current frame.
#[derive(Clone, Copy, Default)]
struct Hands {
    /// The pointer on the 1024x768 screen; off it with `reload`.
    x: f32,
    y: f32,
    trigger: bool,
    trigger_pressed: bool,
    punch: bool,
    throw: bool,
    /// The hand of the next punch (alternating).
    punch_left: bool,
    /// The steering wheel's angle (radians, positive right).
    steer: f32,
}

static HANDS: Mutex<[Hands; PLAYERS]> = Mutex::new([Hands {
    x: 512.0,
    y: 384.0,
    trigger: false,
    trigger_pressed: false,
    punch: false,
    throw: false,
    punch_left: false,
    steer: 0.0,
}; PLAYERS]);

/// Steering (the game's angle of the line between the player's hands): the d-pad turns the
/// wheel toward `STEER_MAX` at `STEER_SPEED` radians per second, release brings it back at
/// `STEER_RETURN`. Beyond 1.05 the game takes a hard turn.
const STEER_MAX: f32 = 1.2;
const STEER_SPEED: f32 = 2.4;
const STEER_RETURN: f32 = 4.0;

static LAST_UPDATE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// The player's left and right controllers.
fn controllers(player: usize) -> (usize, usize) {
    (2 * player, 2 * player + 1)
}

/// DebugGameIO's update (thiscall): the frame's motion codes from the players' guns.
unsafe extern "thiscall" fn debug_update(this: *mut u8) {
    let codes = this.wrapping_add(0x2c) as *mut [i32; 4];
    let previous = this.wrapping_add(0x3c) as *mut [i32; 4];
    let mut new = [0i32; 4];
    let sticks: Vec<_> = (0..PLAYERS).map(wal_payload_common::input).collect();
    let dt = {
        let mut last = LAST_UPDATE.lock().unwrap();
        let now = std::time::Instant::now();
        let dt = last.map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.1));
        *last = Some(now);
        dt
    };
    let mut hands = HANDS.lock().unwrap();
    with_board(|b| {
        for (player, stick) in sticks.iter().enumerate() {
            let mut now = Vec::new();
            b.maps[player].for_each_pressed(stick, |n| now.push(n));
            let h = &mut hands[player];
            let fx = (stick.axis(wal_protocol::Axis::LeftX) as f32 + 32768.0) / 65535.0;
            let fy = (stick.axis(wal_protocol::Axis::LeftY) as f32 + 32768.0) / 65535.0;
            h.x = fx.clamp(0.0, 1.0) * 1024.0;
            h.y = fy.clamp(0.0, 1.0) * 768.0;
            // reload: the gun points off the screen while the button is held
            if now.contains(&Native::Reload) {
                h.x = -100.0;
            }
            let trigger = now.contains(&Native::Trigger);
            h.trigger_pressed = trigger && !h.trigger;
            h.trigger = trigger;
            let (left, right) = controllers(player);
            let punch = now.contains(&Native::Punch);
            if punch && !h.punch {
                let hand = if h.punch_left { left } else { right };
                h.punch_left = !h.punch_left;
                new[hand] = if fy < 0.5 { ST_UP_HARD } else { ST_DOWN_HARD };
            }
            h.punch = punch;
            if now.contains(&Native::Guard) {
                new[left] = GUARD;
                new[right] = GUARD;
            }
            let throw = now.contains(&Native::Throw);
            if throw {
                new[right] = HOLD;
            } else if h.throw {
                new[right] = THROW;
            }
            h.throw = throw;
            if now.contains(&Native::Special) {
                new[left] = SPECIAL;
                new[right] = SPECIAL;
            }
            let dir = now.contains(&Native::SteerRight) as i32 - now.contains(&Native::SteerLeft) as i32;
            h.steer = if dir != 0 {
                (h.steer + dir as f32 * STEER_SPEED * dt).clamp(-STEER_MAX, STEER_MAX)
            } else if h.steer > 0.0 {
                (h.steer - STEER_RETURN * dt).max(0.0)
            } else {
                (h.steer + STEER_RETURN * dt).min(0.0)
            };
        }
    });
    unsafe {
        previous.write_unaligned(codes.read_unaligned());
        codes.write_unaligned(new);
        (this.wrapping_add(0x50) as *mut i32).write_unaligned(1);
    }
}

/// The player of controller `i`.
fn player_of(i: u32) -> Option<usize> {
    let p = i as usize / 2;
    (p < PLAYERS).then_some(p)
}

/// The aim of controller `i` (thiscall, `out` x, y, z): the player's pointer for the right hand
/// (the gun's), 0 (not used) for the left one, as the game's debug input does for the
/// controllers it does not drive (a used left hand: "USE RIGHT" on the right-hand screens).
unsafe extern "thiscall" fn debug_position(_this: *mut u8, i: u32, out: *mut [f32; 3]) -> i32 {
    let pos = player_of(i).filter(|&p| i as usize == controllers(p).1).map(|p| {
        let h = HANDS.lock().unwrap()[p];
        [h.x, h.y, 0.0]
    });
    if !out.is_null() {
        unsafe { out.write_unaligned(pos.unwrap_or([0.0; 3])) };
    }
    1
}

/// Controller `i`'s trigger held (thiscall): the player's right hand holds the gun.
unsafe extern "thiscall" fn debug_trigger(_this: *mut u8, i: u32) -> i32 {
    match player_of(i) {
        Some(p) if i as usize == controllers(p).1 => HANDS.lock().unwrap()[p].trigger as i32,
        _ => 0,
    }
}

/// Controller `i`'s trigger pressed this frame (thiscall).
unsafe extern "thiscall" fn debug_trigger_pressed(_this: *mut u8, i: u32) -> i32 {
    match player_of(i) {
        Some(p) if i as usize == controllers(p).1 => HANDS.lock().unwrap()[p].trigger_pressed as i32,
        _ => 0,
    }
}

/// The player's steering wheel angle (the controller API's `handle`, no debug override).
extern "stdcall" fn steering(player: u32) -> f32 {
    HANDS.lock().unwrap().get(player as usize).map_or(0.0, |h| h.steer)
}

/// DebugGameIO's functions replaced: address, first bytes, replacement.
fn debug_overrides() -> Vec<(usize, [u8; 6], usize)> {
    vec![
        (0x4cff70, [0x56, 0x6a, 0x4a, 0x8b, 0xf1, 0xe8], debug_update as *const () as usize),
        (0x4d0160, [0x83, 0xc8, 0xff, 0x83, 0x79, 0x50], debug_position as *const () as usize),
        (0x4d00f0, [0x83, 0xc8, 0xff, 0x83, 0x79, 0x50], debug_trigger as *const () as usize),
        (0x4d00c0, [0x83, 0xc8, 0xff, 0x83, 0x79, 0x50], debug_trigger_pressed as *const () as usize),
        (0x4d3230, [0x8b, 0x44, 0x24, 0x04, 0x50, 0xe8], steering as *const () as usize),
    ]
}

/// The flat EZ-USB functions: address, first bytes, replacement.
fn overrides() -> Vec<(usize, [u8; 6], usize)> {
    let f = |p: *const ()| p as usize;
    vec![
        (0x4829f0, [0x56, 0xe8, 0xfa, 0xfd, 0xff, 0xff], f(usb_open as *const ())),
        (0x482a80, [0x83, 0x3d, 0x04, 0x80, 0x89, 0x00], f(usb_done as *const ())),
        (0x482aa0, [0xa1, 0x04, 0x80, 0x89, 0x00, 0x8b], f(usb_done as *const ())),
        (0x482ab0, [0xa1, 0x0c, 0x80, 0x89, 0x00, 0xc3], f(usb_yes as *const ())),
        (0x482ac0, [0xa1, 0x04, 0x80, 0x89, 0x00, 0x8b], f(usb_yes as *const ())),
        (0x482ae0, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_pad_read as *const ())),
        (0x482b00, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done1 as *const ())),
        (0x482b20, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done1 as *const ())),
        (0x482b40, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done1 as *const ())),
        (0x482b60, [0x8b, 0x44, 0x24, 0x04, 0xe9, 0x37], f(usb_coin as *const ())),
        (0x482b70, [0x8b, 0x44, 0x24, 0x04, 0x8b, 0x0d], f(usb_done1 as *const ())),
        (0x482b90, [0xe9, 0x8b, 0x35, 0x00, 0x00, 0xcc], f(usb_done as *const ())),
        (0x482ba0, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done as *const ())),
        (0x482be0, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done as *const ())),
        (0x482bf0, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done1 as *const ())),
        (0x482c10, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done as *const ())),
        (0x482c50, [0x8b, 0x44, 0x24, 0x04, 0x8b, 0x0d], f(usb_done1 as *const ())),
        (0x482c70, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done1 as *const ())),
        (0x482c90, [0x8b, 0x44, 0x24, 0x0c, 0x8b, 0x4c], f(usb_done3 as *const ())),
        (0x482cc0, [0x8b, 0x44, 0x24, 0x10, 0x8b, 0x4c], f(usb_done4 as *const ())),
        (0x482cf0, [0xa1, 0x08, 0x80, 0x89, 0x00, 0x8b], f(usb_done as *const ())),
        (0x482d30, [0x8b, 0x44, 0x24, 0x04, 0xe9, 0x17], f(usb_nothing as *const ())),
    ]
}

/// The boot's data checks of the settings: the clock set (setting 0x1d, 5-2503-0000 "clock
/// setting": the PC's clock is the host's) and the 3D sensor calibrated (setting 0x1c, its test
/// menu's 8-point calibration: the aim is the pointer). Their `jne` (set) become `jmp`.
const CLOCK_CHECK: usize = 0x4c1670;
const CALIBRATION_CHECK: usize = 0x4c16b8;

/// The mission select's missions 1-4 all offered (the 4th, IPO, is locked until the others are
/// cleared): their lock flag is `sete cl` (the mission not open) -> `mov cl, 0`
/// (`WAL_KONAMI_UNLOCK_FINAL=1`, default).
const MISSION_UNLOCK: usize = 0x4b76ff;

/// Writes `new` at `at` when the code there is `expected`.
fn patch(at: usize, expected: &[u8], new: &[u8], what: &str) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    if unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) } != expected {
        log!("deka: unexpected code at {at:#x}, {what} not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const std::ffi::c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const std::ffi::c_void, new.len(), old, &mut old);
    }
}

/// The game is deka.exe.
pub fn detected() -> bool {
    crate::file_name(std::ptr::null_mut()).eq_ignore_ascii_case("deka.exe")
}

pub fn init(module: *mut std::ffi::c_void) {
    let mut hooked = 0;
    for (name, address) in crate::otomedius::exports(module) {
        if name.starts_with("bms_") && unsafe { iat::hook("libbms.dll", &name, address) }.is_some() {
            hooked += 1;
        }
    }
    log!("deka: {hooked} libbms imports answered by the payload");
    let mut patched = 0;
    for (at, expected, target) in overrides().into_iter().chain(debug_overrides()) {
        if unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) } == expected {
            unsafe { crate::f24::jump(at, target) };
            patched += 1;
        } else {
            log!("deka: unexpected code at {at:#x}, EZ-USB function not emulated");
        }
    }
    log!("deka: {patched} EZ-USB functions emulated");
    patch(CLOCK_CHECK, &[0x75, 0x18], &[0xeb, 0x18], "the clock check");
    patch(CALIBRATION_CHECK, &[0x75, 0x18], &[0xeb, 0x18], "the 3D sensor calibration check");
    if !std::env::var("WAL_KONAMI_UNLOCK_FINAL").is_ok_and(|v| v.trim() == "0") {
        patch(MISSION_UNLOCK, &[0x0f, 0x94, 0xc1], &[0xb1, 0x00, 0x90], "the final mission unlock");
        log!("deka: final mission unlocked");
    }
}
