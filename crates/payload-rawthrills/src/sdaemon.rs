//! Raw Thrills `sdaemon.exe` games (one engine): The Fast and the Furious (2004, 2006 build),
//! The Fast and the Furious Drift (2007, 2010 build), The Fast and the Furious Super Bikes
//! (2006, 2010 build), Nicktoons Nitro Racing (2009) and H2O Overdrive (2009, 2010 build).
//! `WAL_RAWTHRILLS_GAME`: `fnf`, `drift`, `superbikes`, `nicktoons`, `h2overdrive`; each
//! game's addresses in [`FNF`] / [`DRIFT`] / [`SUPERBIKES`] / [`NICKTOONS`] (the comments give
//! Super Bikes') and [`H2OVERDRIVE`].
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
//!
//! H2O Overdrive (2010 "Triton" build): a later variant, the JAMMA USB board is a device
//! object the game allocates (its init `0x441ff0`) and opens. The board's `Open`
//! (`0x441c20`, SetupAPI/HID, its I/O thread) is replaced by a fake that keeps the device and
//! marks the board open (field `+0xF0`), and the board's per-frame poll (`0x482b20`) is
//! replaced in place: it fills the reports from the virtual sticks and hands each to the
//! game's report parser (`0x4825F0(eax = device, ecx = report)`), returning 0. Once the board
//! reads as present (`0x74ca94`) the per-frame update no longer calls that poll: it reads the
//! board's state only through `GetStatusBlock` (`0x441b30`), replaced by the in-place poll
//! then the original 62 byte copy (under the board's critical section, the game's import
//! table read at each call: the protected exe fills it after the payload starts).
//!
//! The game's input device (`0x42e490`) copies that state block (`+0xF4..+0x132`) into named
//! channels: `Wheel` (the `0xC0` report's byte 2), `Gas` (the `0xA0` report's byte 2: the
//! throttle lever, neutral in the middle, forward / reverse from the gas / brake), `Start`
//! (also the boost), `View1..3` (the `0xC0` report's byte 3 bits 0x01 / 0x02 / 0x04, active
//! low), `Gear` (its high nibble, active high, unused: sent released), `Test` / `Service`
//! (the `0x80` report's byte 2 bits 0x10 / 0x20, swapped against the older games),
//! `VolumeUp` / `VolumeDown` (its byte 3 bits 0x08 / 0x04, swapped too), `Coin1` / `Coin2` /
//! `Bill` and the keypad: 12 keys in the `0xC0` report's key nibble (0xF none,
//! `+0x11A..+0x125`: the tracks and boats selection). The `0x80` report's brake switch (byte 2
//! bit 0x08) feeds no channel, the `0xE0` report is not used.
//!
//! Service and test presses are checked by the press detector (`0x42d590`), which looks for
//! a press edge in the channel's event ring; the keyboard poller that writes those rings is
//! gated off in this build, so the detector's entry jumps to a trampoline (in the poll
//! function's free tail) writing the virtual service / test buttons' edges to the rings
//! first, the channels' `+0x1c` gate enabled. `GetAsyncKeyState` is hooked too: Home / Insert
//! (`0x24` / `0x2d`) report service, End / Delete (`0x23` / `0x2e`) test, Page Up / Down
//! (`0x21` / `0x22`) the volume, from the virtual buttons.
//!
//! The boot additionally needs the board's PortTalk channel: `0x4425a0` opens
//! `\\.\PortTalk` (the Raw Thrills PortTalk driver, its service started on failure) and must
//! get a handle, then fires `DeviceIoControl` code `0x9C402414` calls without checking them;
//! the driver is absent, so the game's `CreateFileA` / `DeviceIoControl` imports are hooked:
//! the channel opens as a fake device, every control succeeds (output zeroed).
//!
//! Movies: the movie open (`0x501a90`) hands Bink the sound output handle it gets from FMOD as
//! its DirectSound object; under Wine FMOD's output is not DirectSound, Bink broke its
//! caller's `esi` and the attract movies crashed at random (in the game or in DXVK's worker
//! thread). The handle is not fetched (`0x501b70` `je` made `jmp`): Bink opens its own
//! DirectSound.
//!
//! Force feedback: the game's wheel force, a float at `0x792C58` (positive pushing right,
//! written by the race), and its centering spring stiffness (`0x792C54`, 0..1.25), read every
//! frame and sent to the launcher as [`output::FFB_CONSTANT`] / [`output::FFB_SPRING`] (on
//! change and every second).

use std::ptr::{addr_of, addr_of_mut};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState, button};

use crate::common::{FfbSender, KEYPAD_NONE, Site, keypad_key, matches, motor_force_byte, pedal, redirect, set_motor, set_spring, write};

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
    /// No gearbox (karts).
    None,
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
    /// Brake type (1: switch, 0: analog); none: the analog brake only.
    brake_type: Option<usize>,
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
    brake_type: Some(0x51fa30),
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
    brake_type: Some(0x5ab0c4),
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
    brake_type: Some(0x537fa0),
    gears: Gears::Lever { six_speed: Some(0x537fa4) },
    brake_pedal_switch: true,
    ffb: Some((0x48a830, &[0xa1, 0xa4, 0x3c, 0x5a, 0x00, 0x85, 0xc0, 0x75, 0x03])),
};

/// Nicktoons Nitro Racing (2009): a kart, analog gas and brake, no gearbox.
const NICKTOONS: Game = Game {
    name: "nicktoons",
    board_init: (0x4a0cc0, &[0x51, 0xe8, 0x5a, 0xa0, 0x00, 0x00, 0x85, 0xc0]),
    parser: (0x4a0e10, &[0x53, 0x55, 0x56, 0x8b, 0x74, 0x24, 0x10]),
    board_check: (0x494d79, &[0x74, 0x04, 0x0b, 0xc8]),
    base_path: (0x491a40, &[0x8b, 0x44, 0x24, 0x04, 0x57, 0x85, 0xc0]),
    keyboard_held: (0x49541c, &[0x81, 0xf9, 0xff, 0x00, 0x00, 0x00]),
    keyboard_once: (0x49560c, &[0x8a, 0x91, 0x70, 0xc7, 0xe4, 0x01]),
    // `mov dword [0x5eda14], 0x1000`: the dump keeps the board only
    io_flags: IoFlags::Imm((0x444456, &[0xc7, 0x05, 0x14, 0xda, 0x5e, 0x00, 0x00, 0x10, 0x00, 0x00])),
    brake_type: None,
    gears: Gears::None,
    brake_pedal_switch: false,
    ffb: Some((0x485010, &[0xa1, 0xc8, 0x57, 0x64, 0x00, 0x85, 0xc0, 0x75, 0x03])),
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
    let name = name.trim();
    if name.eq_ignore_ascii_case(H2OVERDRIVE.name) {
        h2o_init();
        return;
    }
    let Some(g) = [&SUPERBIKES, &FNF, &DRIFT, &NICKTOONS].into_iter().find(|g| g.name.eq_ignore_ascii_case(name)) else {
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
    if let Some(brake_type) = g.brake_type {
        match std::env::var("WAL_RAWTHRILLS_BRAKE").map(|v| v.to_ascii_lowercase()).as_deref() {
            Ok("analog") => write(brake_type, &[0; 4]),
            Ok("switch") => write(brake_type, &[1, 0, 0, 0]),
            _ => {}
        }
        let brake = unsafe { std::ptr::read_volatile(brake_type as *const u32) };
        log!("{}: {} brake", g.name, if brake == 0 { "analog" } else { "switch" });
    }
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
        Gears::Lever { .. } | Gears::None => {}
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

/// H2O Overdrive (Raw Thrills, 2009, 2010 "Triton" build): the JAMMA USB board is a device
/// object the game allocates and opens; its report parser takes the device and a report, and
/// the game's per-frame update reads the device's state.
struct H2o {
    name: &'static str,
    /// The board's `Open` (thiscall, `ecx` the device, 1 on success): replaced by [`h2o_open`].
    open: Site,
    /// The per-frame board poll: replaced in place by [`h2o_poll_code`]; the free tail of the
    /// function hosts the [`h2o_pressdet_tramp`] trampoline.
    poll: Site,
    /// The report parser, `void(eax = device, ecx = report)`.
    parser: Site,
    /// The board's `GetStatusBlock` (thiscall, `ecx` the device, the state copy destination on
    /// the stack, `ret 4`): the game's per-frame update reads the board state through this
    /// vtable slot unconditionally, so it is replaced by [`h2o_status_block`].
    status: Site,
    /// The press detector, `u32(eax = frame ts, stack: event object)`, `ret 4`: every
    /// service/test press check goes through it, so its entry is jumped to
    /// [`h2o_press_det`] (see [`h2o_pressdet_tramp`]).
    pressdet: Site,
    /// The movie open's `je` around the sound output handle fetched from FMOD
    /// (`FMOD::System::getOutputHandle`), handed to Bink as its DirectSound object: made `jmp`
    /// (0: Bink opens its own DirectSound). Under Wine FMOD's output is not DirectSound, Bink
    /// broke its caller's `esi` and crashed (the attract movies, at random).
    movie_sound: Site,
    /// The game's wheel force float (positive pushes right), its spring stiffness the float
    /// before it (0..1.25).
    ffb: usize,
}

const H2OVERDRIVE: H2o = H2o {
    name: "h2overdrive",
    open: (0x441c20, &[0x55, 0x8b, 0xec, 0x83, 0xe4, 0xf8, 0x81, 0xec, 0x4c, 0x02, 0x00, 0x00]),
    poll: (0x482b20, &[0x83, 0xec, 0x24, 0xa1, 0x38, 0x69, 0x69, 0x00, 0x33, 0xc4]),
    parser: (0x4825f0, &[0x56, 0x8b, 0xf1, 0x80, 0x3e, 0x04, 0x75, 0x5a, 0x0f, 0xb6, 0x4e, 0x01]),
    status: (0x441b30, &[0x51, 0xa1, 0x94, 0xca, 0x74, 0x00]),
    pressdet: (0x42d590, &[0xa1, 0x18, 0xca, 0x6a, 0x00, 0x85, 0xc0]),
    movie_sound: (0x501b70, &[0x74, 0x0b, 0x8d, 0x4c, 0x24, 0x0c]),
    ffb: 0x792c58,
};

/// Board input of H2O Overdrive, as its input device reads the board's state (named channels,
/// `0x42e490`): this build's parser swaps test / service and the volume buttons against the
/// older games', and has no brake switch.
const H2O_NATIVES: &[(&str, Native)] = &[
    ("coin1", Native::Switch(0x01)),
    ("start", Native::Switch(0x02)),
    ("coin2", Native::Switch(0x04)),
    ("test", Native::Switch(0x10)),
    ("service", Native::Switch(0x20)),
    ("bill", Native::Switch(0x80)),
    ("volup", Native::Panel(0x08)),
    ("voldown", Native::Panel(0x04)),
    ("view", Native::Bike(0x01)),
    ("view2", Native::Bike(0x02)),
    ("view3", Native::Bike(0x04)),
];

/// Virtual stick -> board, overridden by the profile `native_map`.
const H2O_DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    // the boost is the START button: its own control too
    ("b3", "start"),
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "view"),
    ("b4", "bill"),
    ("up", "volup"),
    ("down", "voldown"),
];

static H2O_MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static H2O_FFB: OnceLock<Mutex<FfbSender>> = OnceLock::new();
/// The in-place poll's first run, to confirm it is actually called by the game.
static H2O_BUILT: AtomicBool = AtomicBool::new(false);
/// The cabinet keypad on the PC's numeric keypad, off with `WAL_RAWTHRILLS_KEYPAD=0`.
static H2O_KEYPAD: OnceLock<bool> = OnceLock::new();
/// The board's device object (set by [`h2o_open`], read by the in-place poll).
static H2O_DEVICE: AtomicUsize = AtomicUsize::new(0);
/// The three reports filled by [`h2o_build`], handed one by one to the game's parser by the
/// in-place poll.
static mut H2O_REPORT: [u8; 12] = [0; 12];

fn h2o_map() -> &'static ButtonMap<Native> {
    H2O_MAP.get_or_init(|| ButtonMap::new(H2O_NATIVES, H2O_DEFAULT_MAP))
}

fn h2o_keypad() -> &'static bool {
    H2O_KEYPAD.get_or_init(|| std::env::var("WAL_RAWTHRILLS_KEYPAD").map_or(true, |v| v != "0"))
}

/// H2O Overdrive: the board's `Open` faked, its per-frame poll replaced by the reports built
/// from the virtual sticks, its `GetStatusBlock` replaced by the same reports plus the state
/// copy, the press detector jumped to the service / test ring writer, the game's report parser
/// kept.
fn h2o_init() {
    let h = &H2OVERDRIVE;
    let sites = [h.open, h.poll, h.parser, h.status, h.pressdet, h.movie_sound];
    if let Some((site, _)) = sites.iter().find(|s| !matches(**s)) {
        log!("{}: unexpected code at {site:#x}, not this build, left alone", h.name);
        return;
    }
    h2o_map();
    log!("{}: keypad {}", h.name, if *h2o_keypad() { "on the numeric keypad" } else { "off" });
    redirect(h.open.0, h2o_open as *const () as usize);
    write(h.poll.0, &h2o_poll_code());
    // The poll code's free tail: the press detector's trampoline, and the jump to it.
    write(h.poll.0 + 53, &h2o_pressdet_tramp());
    write(h.pressdet.0, &h2o_pressdet_jmp());
    redirect(h.status.0, h2o_status_block as *const () as usize);
    write(h.movie_sound.0, &[0xeb]);
    ptt_init();
    kbd_init();
    log!(
        "{}: JAMMA USB board from the virtual sticks (the board's Open faked, its poll and status block replaced)",
        h.name
    );
}

// ── PortTalk board channel ────────────────────────────────────────────────────────────────────

/// The board's `\\.\PortTalk` handle, the driver being absent: this build's boot
/// (`0x4425a0`) needs `CreateFileA("\\.\PortTalk")` to succeed, its `DeviceIoControl` calls
/// (code `0x9C402414`, fire and forget) answered. The board's state still comes from the
/// replaced poll, not this channel.
const PTT_HANDLE: usize = 0x00C0FFEE;

type CreateFileA = unsafe extern "system" fn(
    name: *const i8,
    access: u32,
    share: u32,
    sa: *const std::ffi::c_void,
    disp: u32,
    flags: u32,
    tmpl: usize,
) -> isize;

type DeviceIoControl = unsafe extern "system" fn(
    handle: usize,
    code: u32,
    in_buf: *const std::ffi::c_void,
    in_size: u32,
    out_buf: *mut std::ffi::c_void,
    out_size: u32,
    bytes_ret: *mut u32,
    overlapped: *mut std::ffi::c_void,
) -> i32;

static PTT_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static PTT_IO_CONTROL: AtomicUsize = AtomicUsize::new(0);

unsafe fn ptt_board_name(name: *const i8) -> bool {
    if name.is_null() {
        return false;
    }
    let Ok(s) = (unsafe { std::ffi::CStr::from_ptr(name).to_str() }) else {
        return false;
    };
    s.eq_ignore_ascii_case("\\\\.\\PortTalk") || s.eq_ignore_ascii_case("PortTalk")
}

/// The board channel's `CreateFileA`: `\\.\PortTalk` opens as a fake device, the rest of the
/// game's files go to the original.
unsafe extern "system" fn ptt_create_file(
    name: *const i8,
    access: u32,
    share: u32,
    sa: *const std::ffi::c_void,
    disp: u32,
    flags: u32,
    tmpl: usize,
) -> isize {
    if unsafe { ptt_board_name(name) } {
        log!("{}: board channel \\\\.\\PortTalk opened (driver faked)", H2OVERDRIVE.name);
        return PTT_HANDLE as isize;
    }
    let f = PTT_CREATE_FILE.load(Ordering::Relaxed);
    unsafe { (std::mem::transmute::<usize, CreateFileA>(f))(name, access, share, sa, disp, flags, tmpl) }
}

/// The board channel's `DeviceIoControl`: every control succeeds, the output zeroed.
unsafe extern "system" fn ptt_device_io_control(
    handle: usize,
    _code: u32,
    _in_buf: *const std::ffi::c_void,
    _in_size: u32,
    out_buf: *mut std::ffi::c_void,
    out_size: u32,
    bytes_ret: *mut u32,
    overlapped: *mut std::ffi::c_void,
) -> i32 {
    if handle != PTT_HANDLE {
        let f = PTT_IO_CONTROL.load(Ordering::Relaxed);
        return unsafe {
            (std::mem::transmute::<usize, DeviceIoControl>(f))(
                handle, _code, _in_buf, _in_size, out_buf, out_size, bytes_ret, overlapped,
            )
        };
    }
    if !out_buf.is_null() && out_size != 0 {
        unsafe { (out_buf as *mut u8).write_bytes(0, out_size as usize) };
    }
    if !bytes_ret.is_null() {
        unsafe { bytes_ret.write_volatile(out_size) };
    }
    1
}

fn ptt_init() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "CreateFileA", ptt_create_file as *const () as usize) } {
        PTT_CREATE_FILE.store(o, Ordering::Relaxed);
    } else {
        log!("{}: CreateFileA not imported, PortTalk not faked", H2OVERDRIVE.name);
        return;
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "DeviceIoControl", ptt_device_io_control as *const () as usize) } {
        PTT_IO_CONTROL.store(o, Ordering::Relaxed);
    }
}

// ── Keyboard: service / test / volume ───────────────────────────────────────────────────────

/// H2O reads service, test and the volume from the keyboard while the game's input object is
/// not yet allocated (the boot and the menus): a per-frame poller (`0x546e60`) polls
/// `GetAsyncKeyState` for a set of keys and writes one event ring per key, and the boot "press
/// service to skip the menu" check reads the ring of the key the poller reads *first*: D
/// (0x24) for service, E (0x23) for test, volume up / down on Up (0x21) / Down (0x22). In this
/// build the poller is gated off, so the rings are written directly from the virtual buttons
/// (see [`h2o_press_det`]); `GetAsyncKeyState` is still hooked so the same virtual buttons
/// answer those keys in the builds where the poller does run — and service / test on Insert
/// (0x2d) / Delete (0x2e), the service-mode remap. Every other polled key is left to the
/// original, logged on first sight.
const VK_VOLUP: i32 = 0x21; // Up
const VK_VOLDOWN: i32 = 0x22; // Down

type GetAsyncKeyState = unsafe extern "system" fn(vk: i32) -> i16;

static KBD_ASYNC_KEY_STATE: AtomicUsize = AtomicUsize::new(0);
/// Last reported state of the four keys, one bit each: 0 service, 1 test, 2 volume up,
/// 3 volume down; the change log below keys off it.
static KBD_STATE: AtomicU32 = AtomicU32::new(0);
/// Which of the four keys the game has polled at least once (same bit layout); a one-shot log
/// confirms the game actually reads them through the hook.
static KBD_SEEN: AtomicU32 = AtomicU32::new(0);
/// Every other polled virtual key, one bit each (256 keys); a one-shot log per key.
static KBD_SEEN_BITS: [AtomicU64; 4] = [
    const { AtomicU64::new(0) },
    const { AtomicU64::new(0) },
    const { AtomicU64::new(0) },
    const { AtomicU64::new(0) },
];

/// Whether `vk` is polled for the first time (one-shot).
fn kbd_first_seen(vk: i32) -> bool {
    if !(0..=255).contains(&vk) {
        return false;
    }
    let (word, bit) = ((vk / 64) as usize, 1u64 << (vk % 64));
    KBD_SEEN_BITS[word].fetch_or(bit, Ordering::Relaxed) & bit == 0
}

/// Whether `native` is among the buttons mapped and pressed on the stick.
fn h2o_native_pressed(stick: &StickState, native: Native) -> bool {
    let mut pressed = false;
    h2o_map().for_each_pressed(stick, |n| {
        if n == native {
            pressed = true;
        }
    });
    pressed
}

/// `GetAsyncKeyState`: the service / test / volume keys report the virtual buttons, every
/// other key the original state (the game reads the high bit, `0x8000`, for "down").
unsafe extern "system" fn kbd_get_async_key_state(vk: i32) -> i16 {
    let p1 = wal_payload_common::input(0);
    let (bit, native, name) = match vk {
        0x24 | 0x2d => (0u32, Native::Switch(0x20), "service"), // D / Insert
        0x23 | 0x2e => (1, Native::Switch(0x10), "test"), // E / Delete
        VK_VOLUP => (2, Native::Panel(0x08), "volume up"),
        VK_VOLDOWN => (3, Native::Panel(0x04), "volume down"),
        _ => {
            if kbd_first_seen(vk) {
                log!("{}: GetAsyncKeyState first polled vk {vk:#04x}", H2OVERDRIVE.name);
            }
            return unsafe {
                (std::mem::transmute::<usize, GetAsyncKeyState>(KBD_ASYNC_KEY_STATE.load(Ordering::Relaxed)))(vk)
            }
        }
    };
    let down = h2o_native_pressed(&p1, native);
    let seen = KBD_SEEN.load(Ordering::Relaxed);
    if seen & (1 << bit) == 0 {
        KBD_SEEN.store(seen | (1 << bit), Ordering::Relaxed);
        log!("{}: keyboard first polled {name} (vk {vk:#04x})", H2OVERDRIVE.name);
    }
    let prev = KBD_STATE.load(Ordering::Relaxed);
    let cur = prev & !(1 << bit) | ((down as u32) << bit);
    if cur != prev {
        KBD_STATE.store(cur, Ordering::Relaxed);
        log!("{}: keyboard {name} now {}", H2OVERDRIVE.name, if down { "down" } else { "up" });
    }
    if down { 0x8000u16 as i16 } else { 0 }
}

fn kbd_init() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("user32.dll", "GetAsyncKeyState", kbd_get_async_key_state as *const () as usize) } {
        KBD_ASYNC_KEY_STATE.store(o, Ordering::Relaxed);
        log!("{}: service / test / volume from the virtual buttons (GetAsyncKeyState hooked)", H2OVERDRIVE.name);
    } else {
        log!("{}: GetAsyncKeyState not imported, service / test / volume not faked", H2OVERDRIVE.name);
    }
}

/// The board's `Open` (thiscall, `ecx` the device object): the device kept (the reports go to
/// its parser), the board marked open (`+0xF0`); the game's I/O thread is not started (no
/// board, the reports come from the virtual sticks).
unsafe extern "thiscall" fn h2o_open(device: *mut u8) -> u32 {
    H2O_DEVICE.store(device as usize, Ordering::SeqCst);
    log!("{}: board opened, device {:#x}", H2OVERDRIVE.name, device as usize);
    unsafe { (device.add(0xf0) as *mut u32).write_volatile(1) };
    1
}

// ── GetStatusBlock: the always-run board state read ─────────────────────────────────────────

/// The board's `GetStatusBlock` is the only board read the game's per-frame update performs
/// unconditionally (the replaced poll is skipped once the board reads as "present"), so the
/// replacement feeds the reports to the game's parser — which updates the device's state
/// block — and then copies the block to the caller exactly as the original does (under the
/// status critical section while the board is "present").
static H2O_STATUS_SEEN: AtomicBool = AtomicBool::new(false);
/// The game's import table entries of `EnterCriticalSection` / `LeaveCriticalSection` (the
/// original `GetStatusBlock` calls through them), read at each call: the protected exe fills
/// its imports after the payload starts.
const H2O_IAT_ENTER_CS: usize = 0x63e2c0;
const H2O_IAT_LEAVE_CS: usize = 0x63e2c4;

type EnterCs = unsafe extern "system" fn(*mut std::ffi::c_void);

unsafe extern "thiscall" fn h2o_status_block(device: *mut u8, out: *mut u8) -> u32 {
    if out.is_null() {
        return 0;
    }
    if !H2O_STATUS_SEEN.swap(true, Ordering::SeqCst) {
        log!(
            "{}: status block hook active, device {:#x} (known {:#x}), gate {}, critical section {:#x} enter {:#x} leave {:#x}",
            H2OVERDRIVE.name,
            device as usize,
            H2O_DEVICE.load(Ordering::Relaxed),
            unsafe { (0x74ca94 as *const u32).read_volatile() },
            unsafe { (0x74cc98 as *const u32).read_volatile() },
            unsafe { (H2O_IAT_ENTER_CS as *const usize).read_volatile() },
            unsafe { (H2O_IAT_LEAVE_CS as *const usize).read_volatile() },
        );
    }
    // The in-place poll: builds the reports and hands each to the parser, which takes its
    // arguments in registers (`eax` = device, `ecx` = report): not callable as a Rust function.
    let poll: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(H2OVERDRIVE.poll.0) };
    unsafe { poll() };
    // The original copy: 15 dwords + a word, under the critical section while the board is
    // "present".
    let gate = unsafe { (0x74ca94 as *const u32).read_volatile() };
    let crit = unsafe { (0x74cc98 as *const u32).read_volatile() } as usize;
    let enter = unsafe { (H2O_IAT_ENTER_CS as *const usize).read_volatile() };
    let leave = unsafe { (H2O_IAT_LEAVE_CS as *const usize).read_volatile() };
    if gate != 0 && crit != 0 && enter != 0 {
        unsafe { (std::mem::transmute::<usize, EnterCs>(enter))(crit as *mut std::ffi::c_void) };
    }
    unsafe {
        let src = device.add(0xf4);
        for i in 0..15 {
            (out.add(i * 4) as *mut u32)
                .write_volatile((src.add(i * 4) as *const u32).read_volatile());
        }
        (out.add(60) as *mut u16).write_volatile((src.add(60) as *const u16).read_volatile());
    }
    if gate != 0 && crit != 0 && leave != 0 {
        unsafe { (std::mem::transmute::<usize, EnterCs>(leave))(crit as *mut std::ffi::c_void) };
    }
    1
}

// ── Press detector: the service / test event rings ──────────────────────────────────────────

/// Every "is service / test pressed" check (the boot "press service to skip the menu", the
/// per-frame service state machine, the in-game service mode) goes through the game's press
/// detector (`0x42d590`), which looks for a press edge written to the event object's ring in
/// the current frame — the game's keyboard poller is the ring's writer, and it is gated off in
/// this build. The trampoline (see [`h2o_pressdet_tramp`]) runs this first: it writes the
/// virtual service / test buttons' edges to the rings exactly like the poller does, and
/// enables the event objects' `+0x1c` gate, zero in this build (the keyboard init that would
/// set it never runs).
static PRESSDET_SVC_LAST: AtomicU8 = AtomicU8::new(0);
static PRESSDET_TEST_LAST: AtomicU8 = AtomicU8::new(0);
/// The frame timestamp the detector bails on while it is 0.
static PRESSDET_TS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn h2o_press_det(obj: usize) {
    let ts = unsafe { (0x6aca18 as *const u32).read_volatile() };
    if ts != 0 && PRESSDET_TS.swap(ts, Ordering::Relaxed) == 0 {
        log!("{}: frame timestamp active ({ts})", H2OVERDRIVE.name);
    }
    if ts == 0 {
        return; // the original detector bails here
    }
    let gi = unsafe { (0x78b064 as *const u32).read_volatile() } as usize;
    // the input device's Service channel (`+0x19cc`, board bit 0x20) and Test channel
    // (`+0x17a4`, bit 0x10)
    let (is_svc, is_test) = if gi != 0 {
        (obj == gi + 0x19cc, obj == gi + 0x17a4)
    } else {
        (obj == 0x78b298, obj == 0x78b070)
    };
    if !is_svc && !is_test {
        return;
    }
    let obj = obj as *mut u8;
    if unsafe { (obj.add(0x1c) as *const u32).read_volatile() } == 0 {
        unsafe { (obj.add(0x1c) as *mut u32).write_volatile(1) };
        log!("{}: enabled event object gate +0x1c (obj {:#x})", H2OVERDRIVE.name, obj as usize);
    }
    let p1 = wal_payload_common::input(0);
    let down = if is_svc {
        h2o_native_pressed(&p1, Native::Switch(0x20))
    } else {
        h2o_native_pressed(&p1, Native::Switch(0x10))
    };
    let last = if is_svc {
        PRESSDET_SVC_LAST.load(Ordering::Relaxed)
    } else {
        PRESSDET_TEST_LAST.load(Ordering::Relaxed)
    };
    if down as u8 == last {
        return;
    }
    // A state edge: advance the ring cursor (wrapping at 0x40) and write {ts, state}, the
    // game's keyboard poller's own sequence (cursor, state, timestamp).
    let cur = unsafe { (obj.add(0x220) as *const u32).read_volatile() };
    let next = if cur + 1 == 0x40 { 0 } else { cur + 1 };
    unsafe {
        (obj.add(0x220) as *mut u32).write_volatile(next);
        (obj.add(0x24 + (next as usize) * 8) as *mut u32).write_volatile(down as u32);
        (obj.add(0x20 + (next as usize) * 8) as *mut u32).write_volatile(ts);
    }
    if is_svc {
        PRESSDET_SVC_LAST.store(down as u8, Ordering::Relaxed);
    } else {
        PRESSDET_TEST_LAST.store(down as u8, Ordering::Relaxed);
    }
    log!(
        "{}: wrote {} press ring (ts {ts}, state {down}, obj {:#x})",
        H2OVERDRIVE.name,
        if is_svc { "service" } else { "test" },
        obj as usize,
    );
}

/// The board's reports for the player's stick (and player 2's coin), the wheel force read
/// from the game: called by the in-place poll before the parser calls (the game's thread).
unsafe extern "C" fn h2o_build() {
    if !H2O_BUILT.swap(true, Ordering::SeqCst) {
        log!("{}: first board poll (device {:#x})", H2OVERDRIVE.name, H2O_DEVICE.load(Ordering::SeqCst));
    }
    // the force: positive pushes right; the spring stiffness just before it (0..1.25)
    let f = unsafe { std::ptr::read_volatile(H2OVERDRIVE.ffb as *const f32) };
    set_motor(-f);
    let stiffness = unsafe { std::ptr::read_volatile((H2OVERDRIVE.ffb - 4) as *const f32) };
    set_spring(stiffness / 1.25);
    H2O_FFB.get_or_init(Default::default).lock().unwrap().poll();
    let p1 = wal_payload_common::input(0);
    let (mut switches, mut panel, mut views) = (0u8, 0u8, 0u8);
    h2o_map().for_each_pressed(&p1, |n| match n {
        Native::Switch(b) => switches |= b,
        Native::Panel(b) => panel |= b,
        Native::Bike(b) => views |= b,
        Native::ShiftUp | Native::ShiftDown => {}
    });
    if wal_payload_common::input(1).pressed(button::COIN) {
        switches |= 0x04;
    }
    let key = if *h2o_keypad() { keypad_key() } else { KEYPAD_NONE };
    let reports = [
        4, 0x80, !switches, !panel,
        4, 0xA0, throttle_lever(&p1), pedal(&p1, Axis::Brake),
        // the view buttons active low, the gear switches (high nibble, active high) released
        4, 0xC0 | key, p1.axis_u8(Axis::LeftX), !views & 0x0f,
    ];
    unsafe {
        std::ptr::copy_nonoverlapping(reports.as_ptr(), addr_of_mut!(H2O_REPORT) as *mut u8, 12);
    }
}

/// The boat's throttle, one lever (the `0xA0` report's byte 2, the "Throttle POT"): neutral at
/// rest in the middle, the gas pushes it forward, the brake pulls it back into reverse.
fn throttle_lever(stick: &StickState) -> u8 {
    let (gas, brake) = (stick.axis(Axis::Accel).max(0) as i32, stick.axis(Axis::Brake).max(0) as i32);
    (128 + gas * 127 / 32767 - brake * 128 / 32767).clamp(0, 255) as u8
}

/// The in-place replacement of the board's per-frame poll (53 bytes, the function is 181):
/// fills the reports, hands each to the game's parser (the device in [`H2O_DEVICE`]), returns
/// 0 (the board "works", no re-`Open`).
fn h2o_poll_code() -> Vec<u8> {
    let at = H2OVERDRIVE.poll.0 as u32;
    let mut code = Vec::with_capacity(53);
    let call = |code: &mut Vec<u8>, to: u32| {
        code.push(0xE8);
        code.extend_from_slice(&(to as i32 - (at + code.len() as u32 + 4) as i32).to_le_bytes());
    };
    call(&mut code, h2o_build as *const () as u32);
    let device = (addr_of!(H2O_DEVICE) as *const u8 as u32).to_le_bytes();
    let report = addr_of_mut!(H2O_REPORT) as *mut u8 as u32;
    for i in 0..3u32 {
        code.extend_from_slice(&[0xA1, device[0], device[1], device[2], device[3]]);
        code.push(0xB9);
        code.extend_from_slice(&(report + i * 4).to_le_bytes());
        call(&mut code, H2OVERDRIVE.parser.0 as u32);
    }
    code.extend_from_slice(&[0x33, 0xC0, 0xC3]);
    code
}

/// The 5-byte `jmp` installed at the press detector's entry (the original `mov eax,
/// ds:0x6aca18` it overwrites is re-executed by the trampoline).
fn h2o_pressdet_jmp() -> Vec<u8> {
    let from = H2OVERDRIVE.pressdet.0 as u32;
    let to = H2OVERDRIVE.poll.0 as u32 + 53;
    let mut code = vec![0xE9u8];
    // rel32 from the end of the 5-byte instruction
    code.extend_from_slice(&((to as i32) - (from + 5) as i32).to_le_bytes());
    code
}

/// The 22-byte trampoline in the poll function's free tail: the event object (the detector's
/// stack argument) handed to [`h2o_press_det`] (cdecl: the argument popped here), then the
/// original entry's `mov eax, ds:0x6aca18` re-executed and the flow continued at `0x42d595`.
fn h2o_pressdet_tramp() -> Vec<u8> {
    let at = H2OVERDRIVE.poll.0 as u32 + 53;
    let mut code = Vec::with_capacity(22);
    code.extend_from_slice(&[0xFF, 0x74, 0x24, 0x04]); // push the event object [esp+4]
    code.push(0xE8);
    code.extend_from_slice(
        &((h2o_press_det as *const () as u32) as i32 - (at + code.len() as u32 + 4) as i32)
            .to_le_bytes(),
    );
    code.extend_from_slice(&[0x83, 0xC4, 0x04]); // add esp, 4
    code.extend_from_slice(&[0xA1, 0x18, 0xCA, 0x6A, 0x00]); // mov eax, ds:0x6aca18
    code.push(0xE9);
    code.extend_from_slice(&(0x42d595i32 - (at + code.len() as u32 + 4) as i32).to_le_bytes());
    code
}
