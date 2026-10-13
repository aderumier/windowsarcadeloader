//! Raw Thrills games keeping their inputs behind `SetControl` (USB boards through libusb or HID),
//! `WAL_RAWTHRILLS_GAME`: `supercars` The Fast and the Furious SuperCars (2014 build), `alpine`
//! Super Alpine Racer (2016, 1.50), `snowboarder` X-Games Snowboarder (2019, 1.23), `snocross`
//! Winter X Games SnoCross (2022, 1.58); each game's sites in its [`Game`] table.
//!
//! The game keeps its inputs behind `SetControl(code, value)` (SuperCars `0x4a9510`, cdecl),
//! the board's named inputs (`start1 view1..3 tunes gear1..4 coin1 coin2 test service bill
//! volup voldown start2 keypad adc1..3`) bound to its codes: switches
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
//! (the run directory's long path overruns the game's path buffers). The removed input init
//! also sets the analog full scale: `0x10042` is supplied as 4095, otherwise the game's
//! normalized steering and pedals divide by zero even after calibration.
//!
//! The ski and SnoCross builds take the game's own path without a board instead ("No I/O
//! Board found. Using keyboard/mouse.": the board init's 3 tries fail, `0x10040` board present
//! reads 0); forcing the board present there waits for it forever. Their input init removed,
//! `SetControl` is fed the same way. Ski games: swing `adc1` (`0x10020`), edge / tilt `adc3`
//! (`0x10022`), each calibrated min / center / max in the test menu (Calibration); views
//! `0x10001` / `0x10002`. SnoCross: handlebar, throttle, brake `adc1..3`, START also the nitro,
//! snowballs `0x10001`, view `0x10004`.
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
    /// USB board present: `mov eax, 1; ret` (none: the game's own no board path, its
    /// "No I/O Board found" message).
    board_on: Option<Site>,
    /// Board message sender: `ret`.
    board_send: Option<Site>,
    /// Base path of the files: `xor eax, eax; ret`.
    base_path: Option<Site>,
    /// Force board's force function (`cdecl void(float)`), replaced by [`motor_force`].
    ffb: Option<Site>,
    /// The game's force float read every poll (address, full scale; positive pushing left).
    ffb_float: Option<(usize, f32)>,
    /// Board inputs and the default virtual stick -> board map.
    natives: &'static [(&'static str, Native)],
    default_map: &'static [(&'static str, &'static str)],
    /// Analog codes sent every poll, from the virtual stick's axes.
    analogs: &'static [(u32, Axis)],
    /// Full scale (`0x10042`) normally set by the removed input init, when needed.
    analog_max: Option<u32>,
    /// A 4 position gear lever (codes `0x10005..0x10008`).
    gears: bool,
    /// Code pressed with the brake pedal past a third of its travel.
    brake_switch: Option<u32>,
    /// Build fixes: code sites rewritten (checked against their expected bytes too).
    fixes: &'static [(Site, &'static [u8])],
}

const SUPERCARS: Game = Game {
    name: "supercars",
    set_control: (0x4a9510, &[0x8b, 0x44, 0x24, 0x04, 0xa9, 0x00, 0x00, 0x01, 0x00]),
    input_init: (0x4741db, &[0xe8, 0xf0, 0x59, 0x03, 0x00]),
    board_on: Some((0x401ab0, &[0xa1, 0xb0, 0x0a, 0x7d, 0x04, 0xa8, 0x03])),
    board_send: Some((0x4c27f0, &[0x81, 0xec, 0x08, 0x04, 0x00, 0x00])),
    base_path: Some((0x4a7ee0, &[0x8b, 0x44, 0x24, 0x04, 0x85, 0xc0, 0x57])),
    ffb: Some((0x402cf0, &[0x83, 0xec, 0x08, 0x83, 0x3d, 0xc4, 0x0a, 0x7d, 0x04, 0x00])),
    ffb_float: None,
    natives: NATIVES,
    default_map: DEFAULT_MAP,
    analogs: &[(0x10020, Axis::LeftX), (0x10021, Axis::Accel), (0x10022, Axis::Brake)],
    analog_max: Some(4095),
    gears: true,
    brake_switch: Some(0x10010),
    fixes: &[],
};

/// Super Alpine Racer (2016 build, 1.50): the skis' swing (`adc1`) and edge (`adc3`), views 1 / 2.
const ALPINE: Game = Game {
    name: "alpine",
    set_control: (0x4bcbf0, &[0x55, 0x8b, 0xec, 0x8b, 0x45, 0x08, 0xa9, 0x00, 0x00, 0x01, 0x00]),
    input_init: (0x414d9c, &[0xe8, 0xaf, 0x84, 0x0a, 0x00]),
    board_on: None,
    board_send: None,
    base_path: None,
    ffb: None,
    ffb_float: None,
    natives: SKI_NATIVES,
    default_map: SKI_DEFAULT_MAP,
    analogs: &[(0x10020, Axis::LeftX), (0x10022, Axis::RightX)],
    analog_max: None,
    gears: false,
    brake_switch: None,
    fixes: &[],
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

/// X-Games Snowboarder (2019 build, 1.23): the board's swing (`adc1`) and tilt (`adc3`), views
/// 1 / 2.
const SNOWBOARDER: Game = Game {
    name: "snowboarder",
    set_control: (0x4a5680, &[0x55, 0x8b, 0xec, 0x8b, 0x45, 0x08, 0xa9, 0x00, 0x00, 0x01, 0x00]),
    input_init: (0x412217, &[0xe8, 0xc4, 0x3a, 0x09, 0x00]),
    ..ALPINE
};

/// Winter X Games SnoCross (2022 build, 1.58): handlebar (`adc1`), throttle (`adc2`), brake
/// (`adc3`); START is also the nitro, button 1 throws snowballs, TUNES (`0x10004`) is the view.
/// The dump carries traps of its protection, undone: the C runtime's heap check made fatal
/// (`0x76df3d`), a `jmp $` once a counter reaches 2 (`0x470de0`), a feature check (`0x7783ad`,
/// made 1). The cabinet mode (flags bit 1: USB restarts and reboots on board errors) is not
/// set, and a graphics crash is skipped (`0x4c7fdb` / `0x4c7fe2`). Without a board this build
/// reboots ("Unable to initialize I/O Board!"): its I/O init result, board missing watchdog and
/// displays are patched. Force feedback: the handlebar force the race computes (`0x99ab6c`,
/// -1.2401..1.2401, positive pushing left), read every poll and sent as `FFB_CONSTANT`.
const SNOCROSS: Game = Game {
    name: "snocross",
    set_control: (0x4cad20, &[0x8b, 0x44, 0x24, 0x04, 0xa9, 0x00, 0x00, 0x01, 0x00]),
    input_init: (0x4157af, &[0xe8, 0x7c, 0x5c, 0x0b, 0x00]),
    board_on: None,
    board_send: None,
    base_path: Some((0x4befc0, &[0x8b, 0x44, 0x24, 0x04, 0x85, 0xc0, 0x57])),
    ffb: None,
    // the handlebar force the race computes (`0x46d170`), -1.2401..1.2401
    ffb_float: Some((0x99ab6c, 1.2401)),
    natives: SNOCROSS_NATIVES,
    default_map: SNOCROSS_DEFAULT_MAP,
    analogs: &[(0x10020, Axis::LeftX), (0x10021, Axis::Accel), (0x10022, Axis::Brake)],
    analog_max: None,
    gears: false,
    brake_switch: None,
    fixes: &[
        ((0x76df3d, &[0x90, 0x90]), &[0x75, 0x0d]),
        ((0x470de0, &[0xeb, 0xfe]), &[0x90, 0x90]),
        ((0x7783ad, &[0x83, 0xe0, 0x01]), &[0x31, 0xc0, 0x40]),
        ((0x46c7b0, &[0x83, 0x0d, 0xa0, 0x90, 0x9b, 0x00, 0x01]), &[0x90; 7]),
        ((0x4c7fdb, &[0x74, 0x25]), &[0x90, 0x90]),
        ((0x4c7fe2, &[0x74]), &[0xeb]),
        // no board: the I/O init's result taken as success (else "Unable to initialize I/O
        // Board! Rebooting..."), the reboot (`0x46f6d0`) made its final idle loop, the "I/O
        // BOARD MISSING" displays and the force board's init return at once
        ((0x471499, &[0x0f, 0x85, 0x99, 0x00, 0x00, 0x00]), &[0xe9, 0x9a, 0x00, 0x00, 0x00, 0x90]),
        ((0x46f6d0, &[0x83, 0xec, 0x14, 0x56, 0x6a, 0x03]), &[0xe9, 0xcb, 0x00, 0x00, 0x00, 0x90]),
        ((0x46d020, &[0x51]), &[0xc3]),
        ((0x46caf0, &[0x83, 0xec, 0x08]), &[0xc3, 0x90, 0x90]),
        ((0x46ffe0, &[0xd9, 0xee]), &[0xc3, 0x90]),
        // the game still reads the PC keyboard's developer keys: Space (`push 0x39`) ended the
        // game, made Escape; the Escape box not shown
        ((0x477327, &[0x6a, 0x39]), &[0x6a, 0x01]),
        ((0x476999, &[0xe8, 0x82, 0xe3, 0xff, 0xff]), &[0x90; 5]),
    ],
};

const SNOCROSS_NATIVES: &[(&str, Native)] = &[
    ("start", Native::Switch(0x10000)),
    ("snowball", Native::Switch(0x10001)),
    ("view", Native::Switch(0x10004)),
    ("coin1", Native::Switch(0x10009)),
    ("coin2", Native::Switch(0x1000a)),
    ("test", Native::Switch(0x1000b)),
    ("service", Native::Switch(0x1000c)),
    ("bill", Native::Switch(0x1000d)),
    ("volup", Native::Switch(0x1000e)),
    ("voldown", Native::Switch(0x1000f)),
];

const SNOCROSS_DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "view"),
    ("b2", "snowball"),
    ("b3", "start"),
    ("b4", "bill"),
    ("up", "volup"),
    ("down", "voldown"),
];

/// Board input of the ski games (the board's named inputs).
const SKI_NATIVES: &[(&str, Native)] = &[
    ("start", Native::Switch(0x10000)),
    ("view1", Native::Switch(0x10001)),
    ("view2", Native::Switch(0x10002)),
    ("view3", Native::Switch(0x10003)),
    ("tunes", Native::Switch(0x10004)),
    ("coin1", Native::Switch(0x10009)),
    ("coin2", Native::Switch(0x1000a)),
    ("test", Native::Switch(0x1000b)),
    ("service", Native::Switch(0x1000c)),
    ("bill", Native::Switch(0x1000d)),
    ("volup", Native::Switch(0x1000e)),
    ("voldown", Native::Switch(0x1000f)),
    ("start2", Native::Switch(0x10010)),
];

const SKI_DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "view1"),
    ("b2", "view2"),
    ("b4", "bill"),
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

/// Patches the game named by `WAL_RAWTHRILLS_GAME`, if it is an H2O board game.
pub(crate) fn init(name: &str) -> bool {
    let Some(g) = [&SUPERCARS, &ALPINE, &SNOWBOARDER, &SNOCROSS].into_iter().find(|g| g.name.eq_ignore_ascii_case(name)) else { return false };
    let mut sites = vec![g.set_control, g.input_init];
    sites.extend(g.board_on.into_iter().chain(g.board_send).chain(g.base_path).chain(g.ffb));
    sites.extend(g.fixes.iter().map(|(site, _)| *site));
    if let Some((site, _)) = sites.iter().find(|s| !matches(**s)) {
        log!("{}: unexpected code at {site:#x}, not this build, left alone", g.name);
        return true;
    }
    MAP.get_or_init(|| ButtonMap::new(g.natives, g.default_map));
    for ((at, _), code) in g.fixes {
        write(*at, code);
    }
    write(g.input_init.0, &[0x90; 5]);
    if let Some(on) = g.board_on {
        write(on.0, &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3]);
    }
    if let Some(send) = g.board_send {
        write(send.0, &[0xc3]);
    }
    if let Some(base) = g.base_path {
        write(base.0, &[0x33, 0xc0, 0xc3]);
    }
    if let Some(ffb) = g.ffb {
        redirect(ffb.0, motor_force as *const () as usize);
        log!("{}: force feedback from the force board's force", g.name);
    }
    let set_control: unsafe extern "C" fn(u32, u32) -> u32 = unsafe { std::mem::transmute(g.set_control.0) };
    let keypad = std::env::var("WAL_RAWTHRILLS_KEYPAD").map_or(true, |v| v != "0");
    std::thread::spawn(move || run(g, set_control, keypad));
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

fn run(g: &'static Game, set_control: unsafe extern "C" fn(u32, u32) -> u32, keypad: bool) {
    let map = MAP.get().expect("map set by init");
    let mut sent = Sent::default();
    let mut gear = 0usize;
    let (mut up_held, mut down_held) = (false, false);
    let mut ffb = FfbSender::default();
    loop {
        if let Some((at, scale)) = g.ffb_float {
            set_motor(unsafe { std::ptr::read_volatile(at as *const f32) } / scale);
        }
        ffb.poll();
        let stick = wal_payload_common::input(0);
        let p2 = wal_payload_common::input(1);
        let (mut switches, mut up, mut down, mut lever) = (0u32, false, false, None);
        map.for_each_pressed(&stick, |n| match n {
            Native::Switch(code) => match GEARS.iter().position(|gear| *gear == code).filter(|_| g.gears) {
                Some(i) => lever = Some(i),
                None => switches |= 1 << (code - 0x10000),
            },
            Native::ShiftUp => up = true,
            Native::ShiftDown => down = true,
        });
        if p2.pressed(button::COIN) {
            switches |= 1 << 0xa;
        }
        if let Some(code) = g.brake_switch.filter(|_| stick.axis(Axis::Brake) > BRAKE_SWITCH) {
            switches |= 1 << (code - 0x10000);
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
                if (g.gears && GEARS.contains(&(0x10000 + n))) || (switches ^ sent.switches) & bit == 0 {
                    continue;
                }
                set_control(0x10000 + n, (switches & bit != 0) as u32);
            }
            sent.switches = switches;
            if g.gears && sent.gear != Some(gear) {
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
            if let Some(maximum) = g.analog_max {
                set_control(0x10042, maximum);
            }
            for (code, axis) in g.analogs {
                set_control(*code, analog(&stick, *axis));
            }
        }
        std::thread::sleep(POLL);
    }
}

/// An axis as the board's 12-bit value (wheel 2048 centered, pedals 1 released).
fn analog(stick: &StickState, axis: Axis) -> u32 {
    let v = stick.axis_u16(axis) as u32 >> 4;
    if axis.is_pedal() { v.max(1) } else { v }
}
