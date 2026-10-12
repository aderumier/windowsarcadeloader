//! Dirty Drivin' (2011 build 1.14), `WAL_RAWTHRILLS_GAME=dirtydrivin`.
//!
//! Without its cabinet hardware (the Raw Thrills HID board, the FTDI coin board, PortTalk) the
//! game drives the truck from its developer keyboard device (`0x8ad0b0`): held keys ramp its
//! steering (`+0x4c`, -1 left..1 right) and throttle (`+0x48`, 0..1) floats. Those ramps are
//! removed and a thread of ours writes the floats from the virtual stick (throttle minus
//! brake, as the cabinet's).
//!
//! The rest goes where the cabinet's would:
//! * START and the crank are the developer key Space, VIEW is V: the game polls its keys with
//!   `GetAsyncKeyState`, hooked in its imports (every other key reads up: the PC's keyboard
//!   no longer drives the game).
//! * coins through the coin board's callback (`0x41eef0`, by type: coin 1, coin 2, bill),
//!   which counts them in the audits and credits them;
//! * TEST, SERVICE, volume up / down through the switch latches its switch table takes every
//!   frame (`0x98c490..`, to `0x98c380..`).
//!
//! Build fixes: the hardware init result ignored, no reboot, no watchdog/Watson, no hang on
//! a missing device.
//!
//! Force feedback: the race's wheel force (`0x886ec4`, positive pushing right but on the
//! mirrored tracks 8-14, `0x96f770`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, button};

use crate::common::{FfbSender, Site, matches, set_motor, write};

/// Checked sites and their replacement.
const PATCHES: &[(Site, &[u8])] = &[
    // the cabinet hardware init result ignored
    ((0x403b00, &[0x75, 0x3b]), &[0xeb, 0x3b]),
    ((0x465447, &[0x8b, 0x54, 0x24, 0x10]), &[0xeb, 0x0b, 0x90, 0x90]),
    // no hang on a missing device (`jmp $`)
    ((0x4bc9e8, &[0x75, 0x08]), &[0xeb, 0x08]),
    ((0x5b5bf5, &[0x74, 0x1c]), &[0x90, 0x90]),
    ((0x639b5c, &[0x75, 0x2a]), &[0xeb, 0x2a]),
    ((0x6f236e, &[0x8b, 0x46, 0x10]), &[0x90, 0x90, 0x90]),
    ((0x6fb88f, &[0x75, 0x07]), &[0x90, 0x90]),
    // the developer keyboard's throttle and steering ramps: the floats are ours
    ((0x4514bb, &[0xd9, 0x44, 0x24, 0x08]), &[0x90; 4]),
    ((0x4514c0, &[0xd9, 0x5f, 0x48]), &[0x90; 3]),
    ((0x45159a, &[0xd9, 0x04, 0x24, 0xd9, 0x59, 0x4c]), &[0x90; 6]),
    ((0x4515ac, &[0xd9, 0x04, 0x24, 0xd9, 0x59, 0x4c]), &[0x90; 6]),
];

/// The developer keyboard's throttle and steering floats.
const THROTTLE: usize = 0x8ad0f8;
const STEERING: usize = 0x8ad0fc;
/// The coin board's callback: `cdecl (type)`, 0 coin 1, 1 coin 2, 2 bill.
const COIN: usize = 0x41eef0;
/// Switch latches: TEST, SERVICE, volume up, volume down.
const LATCHES: usize = 0x98c490;
/// The race's wheel force, the track played.
const FFB: usize = 0x886ec4;
const TRACK: usize = 0x96f770;

const VK_SPACE: i32 = 0x20;
const VK_V: i32 = 0x56;

/// Board input.
#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A developer key (virtual key).
    Key(i32),
    /// A coin type.
    Coin(u8),
    /// A switch latch (index).
    Latch(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Key(VK_SPACE)),
    ("crank", Native::Key(VK_SPACE)),
    ("view", Native::Key(VK_V)),
    ("coin1", Native::Coin(0)),
    ("coin2", Native::Coin(1)),
    ("bill", Native::Coin(2)),
    ("test", Native::Latch(0)),
    ("service", Native::Latch(1)),
    ("volup", Native::Latch(2)),
    ("voldown", Native::Latch(3)),
];

/// Virtual stick -> board, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "view"),
    ("b2", "crank"),
    ("b4", "bill"),
    ("up", "volup"),
    ("down", "voldown"),
];

const POLL: Duration = Duration::from_millis(4);

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
/// Developer keys held (bit 0 Space, bit 1 V).
static KEYS: AtomicU32 = AtomicU32::new(0);

/// Patches Dirty Drivin' when `WAL_RAWTHRILLS_GAME=dirtydrivin`.
pub(crate) fn init(name: &str) -> bool {
    if !name.eq_ignore_ascii_case("dirtydrivin") {
        return false;
    }
    if let Some(((site, _), _)) = PATCHES.iter().find(|(s, _)| !matches(*s)) {
        log!("dirtydrivin: unexpected code at {site:#x}, not this build, left alone");
        return true;
    }
    for ((at, _), code) in PATCHES {
        write(*at, code);
    }
    if unsafe { wal_payload_common::iat::hook("user32.dll", "GetAsyncKeyState", get_async_key_state as *const () as usize) }.is_none() {
        log!("dirtydrivin: GetAsyncKeyState not imported, no START / VIEW");
    }
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    std::thread::spawn(run);
    log!("dirtydrivin: inputs from the virtual sticks, keyboard controls off");
    true
}

/// The game's key poll: Space and V as the board's buttons, every other key up.
unsafe extern "system" fn get_async_key_state(vk: i32) -> i16 {
    let bit = match vk {
        VK_SPACE => 1,
        VK_V => 2,
        _ => return 0,
    };
    if KEYS.load(Ordering::Relaxed) & bit != 0 { i16::MIN } else { 0 }
}

fn run() {
    let map = MAP.get().expect("map set by init");
    let coin: unsafe extern "C" fn(u32) = unsafe { std::mem::transmute(COIN) };
    let mut coins_held = [false; 3];
    let mut ffb = FfbSender::default();
    loop {
        let stick = wal_payload_common::input(0);
        let mut keys = 0;
        let mut coins = [false; 3];
        let mut latches = [false; 4];
        map.for_each_pressed(&stick, |n| match n {
            Native::Key(VK_SPACE) => keys |= 1,
            Native::Key(_) => keys |= 2,
            Native::Coin(c) => coins[c as usize] = true,
            Native::Latch(l) => latches[l as usize] = true,
        });
        if wal_payload_common::input(1).pressed(button::COIN) {
            coins[1] = true;
        }
        KEYS.store(keys, Ordering::Relaxed);
        for (c, (&down, held)) in coins.iter().zip(coins_held.iter_mut()).enumerate() {
            if down && !*held {
                unsafe { coin(c as u32) };
            }
            *held = down;
        }
        for (i, &down) in latches.iter().enumerate() {
            if down {
                unsafe { ((LATCHES + i * 4) as *mut u32).write_volatile(1) };
            }
        }
        let steering = stick.axis(Axis::LeftX) as f32 / 32768.0;
        let throttle = (stick.axis(Axis::Accel).max(0) as f32 - stick.axis(Axis::Brake).max(0) as f32) / 32767.0;
        unsafe {
            (STEERING as *mut f32).write_volatile(steering);
            (THROTTLE as *mut f32).write_volatile(throttle);
        }
        let force = unsafe { (FFB as *const f32).read_volatile() };
        let mirrored = (8..=14).contains(&unsafe { (TRACK as *const u8).read_volatile() });
        set_motor(if mirrored { force } else { -force });
        ffb.poll();
        std::thread::sleep(POLL);
    }
}
