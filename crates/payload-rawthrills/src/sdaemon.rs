//! Raw Thrills `sdaemon.exe` games (one engine): The Fast and the Furious (2004, 2006 build),
//! The Fast and the Furious Drift (2007, 2010 build) and The Fast and the Furious Super Bikes
//! (2006, 2010 build). `WAL_RAWTHRILLS_GAME`: `fnf`, `drift`, `superbikes`; each game's
//! addresses in [`FNF`] / [`DRIFT`] / [`SUPERBIKES`] (the comments give Super Bikes').
//!
//! The game reads its JAMMA USB board in an I/O thread: each 4-byte report goes to the parser
//! (`0x4c4900(report)`), which keeps a held flag and a pressed-once latch per board input
//! (button codes `0x1000..0x1010` of the game's input functions) and the analog bytes. Reports
//! (`byte 0` = 4, `byte 1` = kind):
//!
//! | kind | byte 2 | byte 3 |
//! |---|---|---|
//! | `0x80` | switches, active low: 0x01 coin 1, 0x02 start, 0x04 coin 2, 0x08 brake / handbrake (`0x1010`), 0x10 service, 0x20 test, 0x80 bill | active low: 0x04 volume up, 0x08 volume down |
//! | `0xA0` | gas (0 released) | brake |
//! | `0xE0` | DIP switches, active low: 0x80 networking (switch 8), 0x07 machine ID - 1 | 1, as the dump sets it |
//! | `0xC0` + keypad key (0xF none) | wheel / handlebar (0x80 centered) | active low: 0x01 view, 0x02 button 2, 0x04 button 3, 0x08 tunes; active high: 0x10..0x80 gear switches 1-4 (`0x1005..0x1008`) |
//!
//! The board is not opened: its init (`0x4c47b0`: open, I/O thread) is replaced by a thread of
//! ours handing the game's parser the reports built from the virtual sticks, and the board
//! status check of the per-frame update (`0x4b7949`) always finds it working (the analog bytes
//! are then read as on the cabinet).
//!
//! DIP switches: `WAL_RAWTHRILLS_DIPS` (hex, default 0: networking off, machine 1). Before the
//! board answers they read 0xFF: networking on, and the game waited for its linked cabinets.
//!
//! Pedals released read 1, not 0: the calibration (test menu) takes the smallest throttle seen
//! other than 0 as the minimum (a real throttle never rests at 0), "Max Must Be Greater Than
//! Min" otherwise.
//!
//! The base path of the game's files (`0x4b37a0`, NULL: the current directory) is prefixed to
//! its file names in fixed-size stack buffers: the run directory's path overran them
//! ("Checking files", then a return into the text of a file name). Left empty: file names
//! relative to the current directory.
//!
//! The dumps enable DirectInput keyboard and mouse controls (IO flags `0x700`): set back to the
//! board only (`0x1000`), and the game's keyboard input codes (developer keys: Space ends the
//! race...) always read released.
//!
//! Brake type (Super Bikes `0x51fa30`): 1 the brake is the switch (`0x1010`), 0 the analog byte.
//! `WAL_RAWTHRILLS_BRAKE=analog|switch` sets it, else the game's (Super Bikes: switch, The Fast
//! and the Furious: analog, `0x1010` its HANDBRAKE button).
//!
//! Gears (manual transmission):
//! * Super Bikes (`0x45c600`): SHIFT (`0x1005`) pressed shifts up; the cabinet has no shift down
//!   (the game shifts down when braking), only its developer keyboard's Down (`push 0xd0` at
//!   `0x45c650`), made input `0x1006`: `shiftdown`. `shiftup` is SHIFT, also the garage's bike
//!   colour. Out of a race (game state `0x57eb88` not 0xB / 0xC), `shiftdown` holds the brake:
//!   the garage's neon colour cycles while the brake is held (TUNES picks the character). The
//!   brake pedal presses the brake switch past a third of its travel.
//! * The Fast and the Furious: a 4 position gear lever, the race takes the gear of the switch
//!   held (`gear1`..`gear4`: a real H shifter); `shiftup` / `shiftdown` move a lever kept here
//!   (gear 1 at start).
//! * Drift: the same, 4 positions, or with the game's `-6speed` (setting `0x537fa4` = 0) a 6
//!   speed gate from switch pairs: `0x1005` / `0x1006` the left / right column, `0x1007` /
//!   `0x1008` the upper / lower row (1st left up, 2nd right up, 3rd left, 4th right, 5th left
//!   down, 6th right down).
//!
//! Force feedback (The Fast and the Furious, Drift): the game drives the wheel motor through the
//! parallel port (PortTalk driver, absent here: the port stays disabled), its force
//! (`0x46de10(float -1..1)` in The Fast and the Furious) sent as one byte (0x00-0x7F: force >= 0, pushing left; 0x80-0xFF
//! pushing right). That function is replaced: the force goes to the launcher as
//! [`output::FFB_CONSTANT`] (-force x 10000, on change and every second), no port I/O.
//!
//! Player 2's coin is the second coin slot. The cabinet's keypad (12 keys: 1-9, *, 0, #) is
//! the PC's numeric keypad (NumLock on): `.` or `*` for *, `+` or Enter for #
//! (`WAL_RAWTHRILLS_KEYPAD=0`: none).

use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState, button};

use crate::common::{FfbSender, KEYPAD_NONE, Site, keypad_key, matches, motor_force_byte, pedal, redirect, write};

/// How the dump sets its IO flags (DirectInput, keyboard, mouse), turned into the board only.
enum IoFlags {
    /// An instruction writing them: removed (the flags keep the board, `0x1000`).
    Write(Site),
    /// An instruction storing them in `var`: removed, `var` set to the board.
    Store(Site, usize),
    /// `mov dword [var], imm32` writing them: its immediate made the board.
    Imm(Site),
}

enum Gears {
    /// SHIFT shifts up, the developer key Down (`push 0xd0`) made input 0x1006; the game
    /// state (0xB / 0xC in a race).
    Sequential { shift_down_key: Site, game_state: usize },
    /// The race takes the gear of the gear switch held; `six_speed`: setting, 0 a 6 speed gate.
    Lever { six_speed: Option<usize> },
}

struct Game {
    name: &'static str,
    /// JAMMA USB board init (`push ecx; call open; test eax, eax`).
    board_init: Site,
    /// The report parser, `cdecl void(u8 *report)`.
    parser: Site,
    /// `je` (board absent) of the board status check: removed.
    board_check: Site,
    /// Base path of the files (`cdecl void(char *path)`): `xor eax, eax; ret`.
    base_path: Site,
    /// Keyboard branch of the held and pressed-once input functions: `xor eax, eax; ret`.
    keyboard_held: Site,
    keyboard_once: Site,
    io_flags: IoFlags,
    /// Brake type (1: switch, 0: analog).
    brake_type: usize,
    gears: Gears,
    /// The brake pedal also presses the brake switch.
    brake_pedal_switch: bool,
    /// Wheel motor force function (`cdecl u8(float)`), replaced by [`motor_force_byte`].
    ffb: Option<Site>,
}

const SUPERBIKES: Game = Game {
    name: "superbikes",
    board_init: (0x4c47b0, &[0x51, 0xe8, 0x1a, 0x58, 0x00, 0x00, 0x85, 0xc0]),
    parser: (0x4c4900, &[0x53, 0x55, 0x56, 0x8b, 0x74, 0x24, 0x10]),
    board_check: (0x4b7949, &[0x74, 0x04, 0x0b, 0xc8]),
    base_path: (0x4b37a0, &[0x8b, 0x44, 0x24, 0x04, 0x57, 0x85, 0xc0]),
    keyboard_held: (0x4b8006, &[0x81, 0xf9, 0xff, 0x00, 0x00, 0x00]),
    keyboard_once: (0x4b81df, &[0x8a, 0x91, 0xc8, 0x43, 0x59, 0x03]),
    // the dump's `mov dword ss:[0x554504], 0x700` after the game's 0x1000
    io_flags: IoFlags::Write((0x44aaf6, &[0x36, 0xc7, 0x05, 0x04, 0x45, 0x55, 0x00, 0x00, 0x07, 0x00, 0x00])),
    brake_type: 0x51fa30,
    gears: Gears::Sequential { shift_down_key: (0x45c650, &[0x68, 0xd0, 0x00, 0x00, 0x00]), game_state: 0x57eb88 },
    brake_pedal_switch: true,
    ffb: None,
};

const FNF: Game = Game {
    name: "fnf",
    board_init: (0x486800, &[0x51, 0xe8, 0x8a, 0x8e, 0x01, 0x00, 0x85, 0xc0]),
    parser: (0x486950, &[0x53, 0x55, 0x57, 0x8b, 0x7c, 0x24, 0x10]),
    board_check: (0x4834b9, &[0x74, 0x04, 0x0b, 0xc8]),
    base_path: (0x4769e0, &[0x8b, 0x44, 0x24, 0x04, 0x57, 0x85, 0xc0]),
    keyboard_held: (0x483b76, &[0x81, 0xf9, 0xff, 0x00, 0x00, 0x00]),
    keyboard_once: (0x483d4f, &[0x8a, 0x91, 0x88, 0x69, 0x5b, 0x03]),
    // `mov [0x535994], esi` (esi = 0x700)
    io_flags: IoFlags::Store((0x440240, &[0x89, 0x35, 0x94, 0x59, 0x53, 0x00]), 0x535994),
    brake_type: 0x5ab0c4,
    gears: Gears::Lever { six_speed: None },
    brake_pedal_switch: false,
    // `mov eax, [port enabled]; test eax, eax; jne`
    ffb: Some((0x46de10, &[0xa1, 0xd4, 0x2e, 0x5b, 0x00, 0x85, 0xc0, 0x75, 0x03])),
};

const DRIFT: Game = Game {
    name: "drift",
    // the dump returns at once (`ret`, was `push ecx`)
    board_init: (0x4d9db0, &[0xc3, 0xe8, 0x4a, 0x65, 0x00, 0x00, 0x85, 0xc0]),
    parser: (0x4d9f00, &[0x53, 0x55, 0x56, 0x8b, 0x74, 0x24, 0x10]),
    board_check: (0x4ccfc9, &[0x74, 0x04, 0x0b, 0xc8]),
    base_path: (0x4c7c60, &[0x8b, 0x44, 0x24, 0x04, 0x57, 0x85, 0xc0]),
    keyboard_held: (0x4cd686, &[0x81, 0xf9, 0xff, 0x00, 0x00, 0x00]),
    keyboard_once: (0x4cd85f, &[0x8a, 0x91, 0x20, 0x59, 0x5b, 0x04]),
    // `mov dword [0x56fc1c], 0x1f00`
    io_flags: IoFlags::Imm((0x4553f6, &[0xc7, 0x05, 0x1c, 0xfc, 0x56, 0x00, 0x00, 0x1f, 0x00, 0x00])),
    brake_type: 0x537fa0,
    gears: Gears::Lever { six_speed: Some(0x537fa4) },
    brake_pedal_switch: true,
    ffb: Some((0x48a830, &[0xa1, 0xa4, 0x3c, 0x5a, 0x00, 0x85, 0xc0, 0x75, 0x03])),
};

/// Gear switches of each gear, 4 position lever.
const LEVER_4: [u8; 4] = [0x10, 0x20, 0x40, 0x80];
/// Drift's 6 speed gate: column (0x10 left, 0x20 right) and row (0x40 up, 0x80 down).
const LEVER_6: [u8; 6] = [0x50, 0x60, 0x10, 0x20, 0x90, 0xa0];

/// Board input: (report, bit), see the module table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    /// `0x80` report byte 2.
    Switch(u8),
    /// `0x80` report byte 3.
    Panel(u8),
    /// `0xC0` report byte 3.
    Bike(u8),
    ShiftUp,
    ShiftDown,
}

const NATIVES: &[(&str, Native)] = &[
    ("coin1", Native::Switch(0x01)),
    ("start", Native::Switch(0x02)),
    ("coin2", Native::Switch(0x04)),
    ("brake", Native::Switch(0x08)),
    ("handbrake", Native::Switch(0x08)),
    ("service", Native::Switch(0x10)),
    ("test", Native::Switch(0x20)),
    ("bill", Native::Switch(0x80)),
    ("volup", Native::Panel(0x04)),
    ("voldown", Native::Panel(0x08)),
    ("view", Native::Bike(0x01)),
    ("button2", Native::Bike(0x02)),
    ("button3", Native::Bike(0x04)),
    ("tunes", Native::Bike(0x08)),
    ("gear1", Native::Bike(0x10)),
    ("gear2", Native::Bike(0x20)),
    ("gear3", Native::Bike(0x40)),
    ("gear4", Native::Bike(0x80)),
    ("shiftup", Native::ShiftUp),
    ("shiftdown", Native::ShiftDown),
];

/// Virtual stick -> board, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "view"),
    ("b2", "tunes"),
    ("b3", "brake"),
    ("b4", "bill"),
    ("b5", "shiftdown"),
    ("b6", "shiftup"),
    ("up", "volup"),
    ("down", "voldown"),
];

/// Bits of the `0xC0` report's byte 3 that are active high (gear switches).
const GEAR_SWITCHES: u8 = 0xf0;
/// Brake pedal travel pressing the brake switch.
const BRAKE_SWITCH: i16 = 32767 / 3;
const POLL: Duration = Duration::from_millis(4);
/// `0xE0` report byte 3, as the dump sets it.
const DIPS_BYTE3: u8 = 0x01;

static GAME: OnceLock<&'static Game> = OnceLock::new();
static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

fn game() -> &'static Game {
    GAME.get().expect("game")
}

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

pub(crate) fn init() {
    let Ok(name) = std::env::var("WAL_RAWTHRILLS_GAME") else { return };
    let Some(g) = [&SUPERBIKES, &FNF, &DRIFT].into_iter().find(|g| g.name.eq_ignore_ascii_case(name.trim())) else {
        log!("sdaemon: unknown WAL_RAWTHRILLS_GAME '{name}'");
        return;
    };
    let mut sites = vec![g.board_init, g.parser, g.board_check, g.base_path, g.keyboard_held, g.keyboard_once];
    if let Gears::Sequential { shift_down_key, .. } = g.gears {
        sites.push(shift_down_key);
    }
    sites.extend(g.ffb);
    if let Some((site, _)) = sites.iter().find(|s| !matches(**s)) {
        log!("{}: unexpected code at {site:#x}, not this build, left alone", g.name);
        return;
    }
    let _ = GAME.set(g);
    map();
    redirect(g.board_init.0, board_init as *const () as usize);
    write(g.board_check.0, &[0x90, 0x90]);
    write(g.base_path.0, &[0x33, 0xc0, 0xc3]);
    write(g.keyboard_held.0, &[0x33, 0xc0, 0xc3]);
    write(g.keyboard_once.0, &[0x33, 0xc0, 0xc3]);
    if let Gears::Sequential { shift_down_key, .. } = g.gears {
        write(shift_down_key.0, &[0x68, 0x06, 0x10, 0x00, 0x00]);
    }
    if let Some(ffb) = g.ffb {
        redirect(ffb.0, motor_force_byte as *const () as usize);
        log!("{}: force feedback from the wheel motor force", g.name);
    }
    match std::env::var("WAL_RAWTHRILLS_BRAKE").map(|v| v.to_ascii_lowercase()).as_deref() {
        Ok("analog") => write(g.brake_type, &[0; 4]),
        Ok("switch") => write(g.brake_type, &[1, 0, 0, 0]),
        _ => {}
    }
    let brake = unsafe { std::ptr::read_volatile(g.brake_type as *const u32) };
    log!("{}: {} brake", g.name, if brake == 0 { "analog" } else { "switch" });
    match g.io_flags {
        IoFlags::Write(site) if matches(site) => write(site.0, &vec![0x90; site.1.len()]),
        IoFlags::Store(site, var) if matches(site) => {
            write(site.0, &vec![0x90; site.1.len()]);
            write(var, &0x1000u32.to_le_bytes());
        }
        IoFlags::Imm(site) if matches(site) => write(site.0 + site.1.len() - 4, &0x1000u32.to_le_bytes()),
        _ => {}
    }
    log!("{}: JAMMA USB board from the virtual sticks, keyboard controls off", g.name);
}

/// The board init (`cdecl`, result unused): no device, our report thread instead of the
/// game's I/O thread.
unsafe extern "C" fn board_init() -> u32 {
    let g = game();
    log!("{}: board init, reports from the virtual sticks", g.name);
    let dips = std::env::var("WAL_RAWTHRILLS_DIPS")
        .ok()
        .and_then(|v| u8::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    let keypad = std::env::var("WAL_RAWTHRILLS_KEYPAD").map_or(true, |v| v != "0");
    log!("{}: DIP switches {dips:#04x}, keypad {}", g.name, if keypad { "on the numeric keypad" } else { "off" });
    std::thread::spawn(move || {
        let parse: unsafe extern "C" fn(*mut u8) = unsafe { std::mem::transmute(g.parser.0) };
        let mut lever = Lever::default();
        let mut ffb = FfbSender::default();
        loop {
            ffb.poll();
            let mut dip_report = [4, 0xE0, !dips, DIPS_BYTE3];
            unsafe { parse(dip_report.as_mut_ptr()) };
            let key = if keypad { keypad_key() } else { KEYPAD_NONE };
            let p1 = wal_payload_common::input(0);
            for mut report in reports(g, &p1, &wal_payload_common::input(1), key, &mut lever) {
                unsafe { parse(report.as_mut_ptr()) };
            }
            std::thread::sleep(POLL);
        }
    });
    0
}

/// The gear lever moved by the shift buttons ([`Gears::Lever`]).
#[derive(Default)]
struct Lever {
    /// Gear, 0 = 1st.
    gear: usize,
    up: bool,
    down: bool,
}

impl Lever {
    /// The gear switches held for `gears` (each gear's switches), after the shift buttons'
    /// presses.
    fn update(&mut self, up: bool, down: bool, gears: &[u8]) -> u8 {
        if up && !self.up {
            self.gear += 1;
        }
        if down && !self.down {
            self.gear = self.gear.saturating_sub(1);
        }
        self.gear = self.gear.min(gears.len() - 1);
        (self.up, self.down) = (up, down);
        gears[self.gear]
    }
}

/// The board's reports for the player's stick (and player 2's coin), `key` the keypad key.
fn reports(g: &Game, stick: &StickState, p2: &StickState, key: u8, lever: &mut Lever) -> [[u8; 4]; 3] {
    let (mut switches, mut panel, mut bike) = (0u8, 0u8, 0u8);
    let (mut up, mut down) = (false, false);
    map().for_each_pressed(stick, |n| match n {
        Native::Switch(b) => switches |= b,
        Native::Panel(b) => panel |= b,
        Native::Bike(b) => bike |= b,
        Native::ShiftUp => up = true,
        Native::ShiftDown => down = true,
    });
    match g.gears {
        Gears::Sequential { game_state, .. } => {
            // SHIFT; in a race input 0x1006, else the brake (the garage's neon colour)
            if up {
                bike |= 0x10;
            }
            if down {
                let racing = matches!(unsafe { std::ptr::read_volatile(game_state as *const u32) }, 0xb | 0xc);
                if racing { bike |= 0x20 } else { switches |= 0x08 }
            }
        }
        // a real H shifter's switches win over the lever kept here
        Gears::Lever { six_speed } if bike & GEAR_SWITCHES == 0 => {
            let six = six_speed.is_some_and(|at| unsafe { std::ptr::read_volatile(at as *const u32) } == 0);
            bike |= lever.update(up, down, if six { &LEVER_6 } else { &LEVER_4 });
        }
        Gears::Lever { .. } => {}
    }
    if p2.pressed(button::COIN) {
        switches |= 0x04;
    }
    if g.brake_pedal_switch && stick.axis(Axis::Brake) > BRAKE_SWITCH {
        switches |= 0x08;
    }
    let bike = (!bike & !GEAR_SWITCHES) | (bike & GEAR_SWITCHES);
    [
        [4, 0x80, !switches, !panel],
        [4, 0xA0, pedal(stick, Axis::Accel), pedal(stick, Axis::Brake)],
        [4, 0xC0 | key, stick.axis_u8(Axis::LeftX), bike],
    ]
}
