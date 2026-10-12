//! Raw Thrills games on the H2O board (USB, libusb): The Fast and the Furious SuperCars (2014
//! build), `WAL_RAWTHRILLS_GAME=supercars`.
//!
//! The game keeps its inputs behind `SetControl(code, value)` (`0x4a9510`, cdecl): switches
//! `0x10000 + n` (held, pressed-once and released-once flags), analog values `0x10020`..
//! `0x10022` (wheel, gas, brake: 0..=4095) and gears `0x10005..0x10008`. A thread of ours calls
//! it with the virtual sticks (switches on change, analog values every poll):
//!
//! | code | input | code | input |
//! |---|---|---|---|
//! | `0x10000` | start | `0x1000a` | coin 2 |
//! | `0x10001` / `0x10002` | buttons 1, 2 | `0x1000b` | test |
//! | `0x10003` | music (the test menu's MUSIC) | `0x1000c` | service |
//! | `0x10004` | button 4 | | |
//! | `0x10005`..`0x10008` | gear lever positions 1-4 | `0x1000d` | bill |
//! | `0x10009` | coin 1 | `0x1000e` / `0x1000f` | volume up / down |
//! | `0x10010` | brake (digital) | `0x10080`.. | keypad 0-9, `0x1008a` *, `0x1008b` # |
//!
//! The board is not used: the input init (`0x4741db`, DirectInput keyboard/mouse and the USB
//! board) is not called, the board counts as present (`0x401ab0` -> 1) and the board message
//! sender (`0x4c27f0`) returns at once. The base path of the files (`0x4a7ee0`) is left empty
//! (the run directory's long path overruns the game's path buffers).
//!
//! Gears: `shiftup` / `shiftdown` move a 4 position lever kept here (gear 1 at start),
//! `gear1`..`gear4` for a real H shifter.
//!
//! Force feedback: the force board's force `0x402cf0(float -1..1)` (parallel port, absent:
//! disabled) is replaced, the force goes to the launcher as `FFB_CONSTANT` (-force x 10000).

use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState, button};

use crate::common::{FfbSender, KEYPAD_NONE, Site, keypad_key, matches, redirect, set_motor, write};

struct Game {
    name: &'static str,
    /// `SetControl(code, value)`.
    set_control: Site,
    /// `call` of the input init: removed.
    input_init: Site,
    /// USB board present: `mov eax, 1; ret`.
    board_on: Site,
    /// Board message sender: `ret`.
    board_send: Site,
    /// Base path of the files: `xor eax, eax; ret`.
    base_path: Site,
    /// Force board's force function (`cdecl void(float)`), replaced by [`motor_force`].
    ffb: Option<Site>,
}

const SUPERCARS: Game = Game {
    name: "supercars",
    set_control: (0x4a9510, &[0x8b, 0x44, 0x24, 0x04, 0xa9, 0x00, 0x00, 0x01, 0x00]),
    input_init: (0x4741db, &[0xe8, 0xf0, 0x59, 0x03, 0x00]),
    board_on: (0x401ab0, &[0xa1, 0xb0, 0x0a, 0x7d, 0x04, 0xa8, 0x03]),
    board_send: (0x4c27f0, &[0x81, 0xec, 0x08, 0x04, 0x00, 0x00]),
    base_path: (0x4a7ee0, &[0x8b, 0x44, 0x24, 0x04, 0x85, 0xc0, 0x57]),
    ffb: Some((0x402cf0, &[0x83, 0xec, 0x08, 0x83, 0x3d, 0xc4, 0x0a, 0x7d, 0x04, 0x00])),
};

/// Board input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    /// A switch, by its `SetControl` code.
    Switch(u32),
    ShiftUp,
    ShiftDown,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Switch(0x10000)),
    ("button1", Native::Switch(0x10001)),
    ("view", Native::Switch(0x10001)),
    ("button2", Native::Switch(0x10002)),
    ("music", Native::Switch(0x10003)),
    ("tunes", Native::Switch(0x10003)),
    ("button3", Native::Switch(0x10003)),
    ("button4", Native::Switch(0x10004)),
    ("gear1", Native::Switch(0x10005)),
    ("gear2", Native::Switch(0x10006)),
    ("gear3", Native::Switch(0x10007)),
    ("gear4", Native::Switch(0x10008)),
    ("coin1", Native::Switch(0x10009)),
    ("coin2", Native::Switch(0x1000a)),
    ("test", Native::Switch(0x1000b)),
    ("service", Native::Switch(0x1000c)),
    ("bill", Native::Switch(0x1000d)),
    ("volup", Native::Switch(0x1000e)),
    ("voldown", Native::Switch(0x1000f)),
    ("brake", Native::Switch(0x10010)),
    ("shiftup", Native::ShiftUp),
    ("shiftdown", Native::ShiftDown),
];

/// Virtual stick -> board, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "button1"),
    ("b2", "music"),
    ("b3", "brake"),
    ("b4", "bill"),
    ("b5", "shiftdown"),
    ("b6", "shiftup"),
    ("b7", "button2"),
    ("b8", "button4"),
    ("up", "volup"),
    ("down", "voldown"),
];

/// Switch codes sent (0x10000..=0x10010).
const SWITCHES: u32 = 0x11;
const GEARS: [u32; 4] = [0x10005, 0x10006, 0x10007, 0x10008];
/// Keypad codes by keypad key index (1-9, *, 0, #).
const KEYPAD_CODES: [u32; 12] = [0x10081, 0x10082, 0x10083, 0x10084, 0x10085, 0x10086, 0x10087, 0x10088, 0x10089, 0x1008a, 0x10080, 0x1008b];
/// Brake pedal travel pressing the digital brake.
const BRAKE_SWITCH: i16 = 32767 / 3;
const POLL: Duration = Duration::from_millis(4);

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// Patches the game named by `WAL_RAWTHRILLS_GAME`, if it is an H2O board game.
pub(crate) fn init(name: &str) -> bool {
    let Some(g) = [&SUPERCARS].into_iter().find(|g| g.name.eq_ignore_ascii_case(name)) else { return false };
    let mut sites = vec![g.set_control, g.input_init, g.board_on, g.board_send, g.base_path];
    sites.extend(g.ffb);
    if let Some((site, _)) = sites.iter().find(|s| !matches(**s)) {
        log!("{}: unexpected code at {site:#x}, not this build, left alone", g.name);
        return true;
    }
    map();
    write(g.input_init.0, &[0x90; 5]);
    write(g.board_on.0, &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3]);
    write(g.board_send.0, &[0xc3]);
    write(g.base_path.0, &[0x33, 0xc0, 0xc3]);
    if let Some(ffb) = g.ffb {
        redirect(ffb.0, motor_force as *const () as usize);
        log!("{}: force feedback from the force board's force", g.name);
    }
    let set_control: unsafe extern "C" fn(u32, u32) -> u32 = unsafe { std::mem::transmute(g.set_control.0) };
    let keypad = std::env::var("WAL_RAWTHRILLS_KEYPAD").map_or(true, |v| v != "0");
    std::thread::spawn(move || run(set_control, keypad));
    log!("{}: H2O board from the virtual sticks, keyboard controls off", g.name);
    true
}

/// The force board's force (`cdecl`, -1..=1, positive pushing left), for the launcher.
unsafe extern "C" fn motor_force(force: f32) {
    set_motor(force);
}

/// Board state sent to the game.
#[derive(Default)]
struct Sent {
    /// Switches held, bit n = code 0x10000 + n.
    switches: u32,
    keypad: Option<u32>,
    gear: Option<usize>,
}

fn run(set_control: unsafe extern "C" fn(u32, u32) -> u32, keypad: bool) {
    let mut sent = Sent::default();
    let mut gear = 0usize;
    let (mut up_held, mut down_held) = (false, false);
    let mut ffb = FfbSender::default();
    loop {
        ffb.poll();
        let stick = wal_payload_common::input(0);
        let p2 = wal_payload_common::input(1);
        let (mut switches, mut up, mut down, mut lever) = (0u32, false, false, None);
        map().for_each_pressed(&stick, |n| match n {
            Native::Switch(code) => match GEARS.iter().position(|g| *g == code) {
                Some(i) => lever = Some(i),
                None => switches |= 1 << (code - 0x10000),
            },
            Native::ShiftUp => up = true,
            Native::ShiftDown => down = true,
        });
        if p2.pressed(button::COIN) {
            switches |= 1 << 0xa;
        }
        if stick.axis(Axis::Brake) > BRAKE_SWITCH {
            switches |= 1 << 0x10;
        }
        // sequential shifting, a real H shifter's switches win
        if up && !up_held {
            gear = (gear + 1).min(GEARS.len() - 1);
        }
        if down && !down_held {
            gear = gear.saturating_sub(1);
        }
        (up_held, down_held) = (up, down);
        let gear = lever.unwrap_or(gear);
        unsafe {
            for n in 0..SWITCHES {
                let bit = 1 << n;
                if GEARS.contains(&(0x10000 + n)) || (switches ^ sent.switches) & bit == 0 {
                    continue;
                }
                set_control(0x10000 + n, (switches & bit != 0) as u32);
            }
            sent.switches = switches;
            if sent.gear != Some(gear) {
                for (i, code) in GEARS.iter().enumerate() {
                    set_control(*code, (i == gear) as u32);
                }
                sent.gear = Some(gear);
            }
            let key = if keypad { keypad_key() } else { KEYPAD_NONE };
            let key = (key != KEYPAD_NONE).then(|| KEYPAD_CODES[key as usize]);
            if key != sent.keypad {
                if let Some(old) = sent.keypad {
                    set_control(old, 0);
                }
                if let Some(new) = key {
                    set_control(new, 1);
                }
                sent.keypad = key;
            }
            set_control(0x10020, analog(&stick, Axis::LeftX));
            set_control(0x10021, analog(&stick, Axis::Accel));
            set_control(0x10022, analog(&stick, Axis::Brake));
        }
        std::thread::sleep(POLL);
    }
}

/// An axis as the board's 12-bit value (wheel 2048 centered, pedals 1 released).
fn analog(stick: &StickState, axis: Axis) -> u32 {
    let v = stick.axis_u16(axis) as u32 >> 4;
    if axis.is_pedal() { v.max(1) } else { v }
}
