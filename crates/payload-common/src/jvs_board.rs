//! JVS I/O board of Taito Type X and Sega RingEdge cabinets (Sega 837-14572 "Type 3"): Taito
//! stick mode (2 players, 16 switches), JVS version 0x30. The options keep their Type X
//! names (`WAL_TYPEX_JVS_*`) on every system.
//!
//! Replies: one report byte for the packet, then each command's data; the commands after
//! the first add their own report byte.
//!
//! Switch byte 1: start 0x80, service 0x40, up 0x20, down 0x10, left 0x08, right 0x04,
//! button 1 0x02, button 2 0x01. Switch byte 2: buttons 3-8 0x80/0x40/0x20/0x10/0x08/0x04.
//! System byte: test 0x80. Coins count up when the coin input is released.
//!
//! `WAL_TYPEX_JVS_DLLS`: comma-separated I/O DLL paths to load and hook before startup
//! (e.g. `bin/xioman.dll` for Source-engine cabinets). Default: executable only.
//!
//! Analog channels: `WAL_TYPEX_JVS_ANALOG` (fixed values, e.g. a volume knob) and
//! `WAL_TYPEX_JVS_ANALOG_INPUTS` (player 1's axes, e.g. the pedals of driving games; `p2:lx`:
//! another player's, e.g. the guns of Transformers: Human Alliance; `lx*0.35`: scaled around the
//! center; `lx*0.73@89C0`: around another center, hex).
//!
//! `WAL_TYPEX_JVS_IDENTIFIER`: another board's identification string (`10`), e.g. Let's Go
//! Safari's `837-14895` in its "FE MODE"; `WAL_TYPEX_JVS_SWITCHES=<players>x<switches>`:
//! another switch layout in the features (default `2x16`; 1 player with 22 switches: 3 bytes,
//! buttons 9-16 in the third).
//!
//! `WAL_TYPEX_JVS_PULSE=<button>,...@<ms>`: those buttons (1-16) pulse while held, on for
//! `ms` then off as long: Let's Go Safari's wheel, a rotary encoder on 2 switches (each pulse
//! turns it a step; toggled on each switch read, it does not turn).
//!
//! `WAL_TYPEX_JVS_TOUCH=<x>,<y>,...`: a touch screen on the screen position input (`25`), the
//! panel's raw values at the picture's top left, top right, bottom left and bottom right corners
//! (4 x, y pairs: the game's default calibration). The touch is player 1's right stick position
//! over the screen (the profile's mouse or gun drives `rx`/`ry`), mapped into the picture
//! (`screenshot::to_picture`: letterboxed, or the game desktop fitted), between the corners; the touch switch is a `native_map`
//! entry (Harikiri Professional Baseball: `b4: btn12`).
//!
//! `WAL_TYPEX_JVS_ROTARY=<n>`: rotary channels in the features, read as 0 (CHUNITHM checks the
//! cabinet board's 4).
//!
//! `WAL_TYPEX_JVS_HELD=btn<n>,...`: player 1 switches always on (buttons 1-16): Usagi Online's
//! touch panel line, without which the game never sees the touch released.
//!
//! `WAL_TYPEX_JVS_LAYOUT=wontertainment`: Won!Tertainment Music Channel's cabinet switches
//! (see `Board::wontertainment`).
//!
//! `WAL_TYPEX_JVS_TICKETS=<output>,btn<n>`: a ticket dispenser: while general purpose output
//! `<output>` (1-24: output 1 is the first byte's 0x80) runs its motor, player 1's switch
//! `btn<n>` (as `WAL_TYPEX_JVS_HELD`) pulses once per ticket, its notch sensor (see
//! [`Tickets`]; Sonic Dash Extreme: `8,btn4`).
//!
//! `WAL_TYPEX_JVS_LAYOUT=k-on`: K-On!'s card dispenser on player 1's switch byte 2 and the
//! general purpose outputs (see [`Dispenser`]).
//!
//! `WAL_TYPEX_JVS_LAYOUT=levers`: Mario & Sonic at the Rio 2016 Olympic Games' two levers, each
//! 8 direction sensors (see [`levers`]).
//!
//! `WAL_TYPEX_JVS_LAYOUT=battle-gear` / `battle-gear-pro`: Battle Gear 4's key reader (Taito
//! commands `6A`-`70`); `-pro` adds the professional cabinet's second board (node 2, its
//! presence makes the game use the wide monitor & clutch mode) and the H shifter (see
//! [`BattleGear`]).
//!
//! `WAL_TYPEX_JVS_LAYOUT=h-shifter`: Initial D ARCADE STAGE Zero's 6-speed H shifter on player
//! 2's directions (its gate switches, payload-common's `hshifter`: b7 / b6 shift up / down,
//! kp1-kp6 the positions).

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use crate::jvs::{Encoder, parse};
use crate::mapping::ButtonMap;
use crate::{drive, log, serial};
use wal_protocol::{Axis, StickState, button};

const IDENTIFIER: &[u8] = b"SEGA CORPORATION;I/O BD JVS;837-14572;Ver1.00;2005/10\0";
const COMMAND_REVISION: u8 = 0x13;
const JVS_VERSION: u8 = 0x30;
const COMM_VERSION: u8 = 0x10;

/// Native JVS inputs of one player.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Start,
    Service,
    Test,
    Coin,
    Up,
    Down,
    Left,
    Right,
    Btn(u8),
    /// Player 2's switch bits (first byte, second byte), from any player's stick: Initial D's
    /// shifter (player 2's directions and buttons) driven from the wheel's own buttons.
    P2(u8, u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("service", Native::Service),
    ("test", Native::Test),
    ("coin", Native::Coin),
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
    ("btn1", Native::Btn(1)),
    ("btn2", Native::Btn(2)),
    ("btn3", Native::Btn(3)),
    ("btn4", Native::Btn(4)),
    ("btn5", Native::Btn(5)),
    ("btn6", Native::Btn(6)),
    ("btn7", Native::Btn(7)),
    ("btn8", Native::Btn(8)),
    // third switch byte of the generic layout (Battle Gear 4: the professional H shifter)
    ("btn9", Native::Btn(9)),
    ("btn10", Native::Btn(10)),
    ("btn11", Native::Btn(11)),
    ("btn12", Native::Btn(12)),
    ("btn13", Native::Btn(13)),
    ("btn14", Native::Btn(14)),
    ("btn15", Native::Btn(15)),
    ("btn16", Native::Btn(16)),
    ("p2start", Native::P2(0x80, 0)),
    ("p2up", Native::P2(0x20, 0)),
    ("p2down", Native::P2(0x10, 0)),
    ("p2left", Native::P2(0x08, 0)),
    ("p2right", Native::P2(0x04, 0)),
    ("p2btn1", Native::P2(0x02, 0)),
    ("p2btn2", Native::P2(0x01, 0)),
    ("p2btn3", Native::P2(0, 0x80)),
    ("p2btn4", Native::P2(0, 0x40)),
    ("p2btn5", Native::P2(0, 0x20)),
    ("p2btn6", Native::P2(0, 0x10)),
    ("p2btn7", Native::P2(0, 0x08)),
    ("p2btn8", Native::P2(0, 0x04)),
];

/// Virtual stick -> JVS; games override with `WAL_MAP` (profile `native_map`).
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("service", "service"),
    ("test", "test"),
    ("coin", "coin"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "btn1"),
    ("b2", "btn2"),
    ("b3", "btn3"),
    ("b4", "btn4"),
    ("b5", "btn5"),
    ("b6", "btn6"),
];

struct Board {
    map: ButtonMap<Native>,
    /// Coin counter of each slot; a player's coins go to their slot, or to the last one the
    /// game reads (`21 nn`) when it reads fewer: one shared coin slot for the 4 guns of Gaia
    /// Attack 4.
    coins: [u16; 4],
    coin_held: [bool; 4],
    coin_slots: usize,
    /// `WAL_TYPEX_JVS_LAYOUT=haunted-museum`: the gun cabinets' switch wiring (Haunted Museum
    /// 1/2) for `20 01 03` (1 player, 3 bytes): P1 start on up 0x20, P2 start on down 0x10;
    /// third byte: service 0x08, P2/P1 action 0x40/0x80 active low (coins: the coin counter). With the generic reply the actions read as held and up/down as
    /// both starts.
    haunted_museum: bool,
    /// `WAL_TYPEX_JVS_LAYOUT=gaia-attack-4`: the Haunted Museum wiring with 4 players (found
    /// with scripted inputs): P1 / P2 start on up 0x20 / down 0x10, P3 / P4 start in the third
    /// byte (0x80 / 0x40, Haunted Museum II's action bits), service 0x08 there. The plain start
    /// bit (0x80) does nothing.
    gaia_attack: bool,
    /// `WAL_TYPEX_JVS_LAYOUT=block-king`: Block King Ball Shooter's switches for `20 01 03`
    /// (found with its switch test): first byte service 0x40, left (P1) start 0x20, right (P2)
    /// start 0x10, cannon 0x08 (button 2); second byte SELECT 0x08 (button 4), ENTER 0x04
    /// (button 3). The standard start bit 0x80 is not read.
    block_king: bool,
    /// `WAL_TYPEX_JVS_LAYOUT=elevator-action`: Elevator Action Death Parade's cabinet for
    /// `20 01 04` (found with its switch test): one block for both guns, each gun's button its
    /// own bit (player 1's button 1 0x02, player 2's 0x01), the elevator panel (up 0x20, action
    /// 0x08 on left, down 0x10), service 0x40, SELECT / ENTER (buttons 7 / 8) from either
    /// player.
    elevator_action: bool,
    /// Cyber Diver's two handle switches are active low (JVS buttons 7/8).
    cyber_diver: bool,
    /// Pleasant Goat: the dispenser-ready signal is switch byte 3 bit 0x40.
    happy_playground: bool,
    /// `WAL_TYPEX_JVS_LAYOUT=levers`: see [`levers`].
    levers: bool,
    /// `WAL_TYPEX_JVS_LAYOUT=h-shifter`: the H shifter's gate on player 2's directions.
    h_shifter: bool,
    /// `WAL_TYPEX_JVS_LAYOUT=k-on`: K-On!'s card dispenser on player 1's switch byte 2 (ready
    /// 0x80, jam 0x40, empty 0x20) and output byte 1 (dispense pulse 0x08): see [`Dispenser`].
    dispenser: Option<Dispenser>,
    /// `WAL_TYPEX_JVS_TICKETS`: a ticket dispenser (see [`Tickets`]).
    tickets: Option<Tickets>,
    /// `WAL_TYPEX_JVS_LAYOUT=wontertainment`: Won!Tertainment Music Channel's cabinet for
    /// `20 02 02` (found with its I/O test): system byte SELECT 0x80; player 1's first byte
    /// ENTER 0x40, door 0x20, the card dispenser's ENABLE 0x10 (on: without it error 100,
    /// "cannot dispense cards"), JAM 0x08, EMPTY 0x04; its second byte the LEFT / RIGHT
    /// buttons 0x80 / 0x40, SERVICE 0x08, CARD OUT 0x04, TEST 0x01. LEFT / RIGHT are buttons
    /// 1 / 2, SELECT / ENTER (operator buttons, the test menu) buttons 5 / 6. After the
    /// dispenser's motor starts (output byte 1 bit 0x10), the gate sensor (ENABLE) goes off
    /// while the card leaves, then CARD OUT is on while it lies in the tray.
    wontertainment: bool,
    /// Its output byte 1 at the last `32` and when the dispenser last dispensed.
    won_output: u8,
    won_dispensed: Option<std::time::Instant>,
    /// `WAL_TYPEX_JVS_ANALOG=v0,v1,...` (hex): fixed analog channel values (default 0).
    analog_fixed: Vec<u16>,
    /// `WAL_TYPEX_JVS_ANALOG_INPUTS=<axis>,...`: player 1's virtual axis read on each
    /// channel (`-axis` inverted; empty: the fixed value). When set, the features report 8
    /// analog channels: games poll `22` only for the channels the board declares (Valve
    /// Limit R: gas on channel 1, brake on channel 2).
    /// `axis*0.35`: scaled around the center (Initial D: the cabinet's steering sensor covers
    /// about a third of the channel, a full PC wheel turn would be far too much).
    /// `axis*0.73@89C0`: around that center (hex), the game's default sensor calibration
    /// (Sonic & Sega All-Stars Racing: steering 0x2C80-0xE700).
    analog_inputs: Vec<Option<(usize, AnalogInput, f32, u16)>>,
    /// `WAL_TYPEX_JVS_TOUCH`: the touch panel's raw corners (top left, top right, bottom left,
    /// bottom right: x, y each).
    touch: Option<[f32; 8]>,
    /// `WAL_TYPEX_JVS_HELD`: player 1's (switch byte 1, 2, 3) bits always on.
    held: (u8, u8, u8),
    /// `WAL_TYPEX_JVS_OUTPUTS=<n>`: general purpose outputs declared in the features (0: none).
    outputs: u8,
    /// `WAL_TYPEX_JVS_ROTARY=<n>`: rotary channels declared in the features (0: none), read as
    /// 0.
    rotary: u8,
    /// `WAL_TYPEX_JVS_IDENTIFIER`: the identification string (default [`IDENTIFIER`]).
    identifier: Vec<u8>,
    /// `WAL_TYPEX_JVS_SWITCHES=<players>x<switches>`: the features' switch layout.
    switches: (u8, u8),
    /// `WAL_TYPEX_JVS_PULSE`: (switch byte 1, 2, 3 masks of the pulsed buttons, half period).
    pulse: Option<([u8; 3], std::time::Duration)>,
    battle_gear: Option<BattleGear>,
    /// Addresses assigned since the last bus reset (2 boards with `battle-gear-pro`).
    addresses: u8,
}

/// The sensors of a lever's 8 directions, clockwise from up (switch byte 1, 2): right, left, up,
/// down, buttons 1-4 (Mario & Sonic at Rio 2016's input test).
const LEVER_SENSORS: [(u8, u8); 8] = [(0x04, 0), (0x08, 0), (0x20, 0), (0x10, 0), (0x02, 0), (0x01, 0), (0, 0x80), (0, 0x40)];

/// Direction (0 up, clockwise in eighths) of a stick position (y down), `None` near the center.
fn lever_direction(x: f32, y: f32) -> Option<usize> {
    if x.hypot(y) < 0.5 {
        return None;
    }
    let turns = x.atan2(-y) / std::f32::consts::TAU;
    Some(((turns * 8.0).round() as i32).rem_euclid(8) as usize)
}

/// The players' switch bytes (switch byte 1, byte 2, test held, coin held, byte 3).
pub type Switches = (u8, u8, bool, bool, u8);

static SWITCHES_HOOK: std::sync::OnceLock<fn(&mut [Switches])> = std::sync::OnceLock::new();

/// A game's own switches over the players' (CHUNITHM's air sensors), set at each switch read.
pub fn set_switches_hook(hook: fn(&mut [Switches])) {
    let _ = SWITCHES_HOOK.set(hook);
}

/// `WAL_TYPEX_JVS_LAYOUT=levers`: Mario & Sonic at the Rio 2016 Olympic Games' cabinet, one
/// player holding two levers (player 1's and player 2's switches), each read by 8 photo sensors
/// around it, one per direction ([`LEVER_SENSORS`]). The left lever is player 1's directions
/// (its d-pad, keys, its left stick through `stick_as_dpad`), the right lever player 2's
/// directions or player 1's right stick. Their buttons, the foot and jump sensors are
/// `native_map` entries (`start`, `btn5`-`btn8`, `p2start`, `p2btn5`-`p2btn8`).
fn levers(players: &mut [(u8, u8, bool, bool, u8)], p1: &StickState) {
    let digital = |b: u8| {
        let axis = |neg: u8, pos: u8| (b & pos != 0) as i32 as f32 - (b & neg != 0) as i32 as f32;
        (axis(0x08, 0x04), axis(0x20, 0x10))
    };
    let (lx, ly) = digital(players[0].0);
    let (mut rx, mut ry) = digital(players[1].0);
    if (rx, ry) == (0.0, 0.0) {
        (rx, ry) = (p1.axis(Axis::RightX) as f32 / 32767.0, p1.axis(Axis::RightY) as f32 / 32767.0);
    }
    for (p, dir) in players.iter_mut().zip([lever_direction(lx, ly), lever_direction(rx, ry)]) {
        p.0 &= !0x3F;
        p.1 &= !0xC0;
        if let Some((b0, b1)) = dir.map(|d| LEVER_SENSORS[d]) {
            p.0 |= b0;
            p.1 |= b1;
        }
    }
}

/// Analog channel source: player 1's axis, inverted (`-axis`) or its positive half only
/// (`+axis`: 0 at the center and below, a clutch on a stick axis).
#[derive(Clone, Copy, Debug)]
enum AnalogInput {
    Axis(Axis),
    Inverted(Axis),
    Positive(Axis),
}

/// K-On!'s card dispenser (`WAL_TYPEX_JVS_LAYOUT=k-on`): the ready line (a card waits at the
/// exit) is player 1's switch byte 2 bit 0x80; the game pulses general purpose output byte 1
/// bit 0x08 to dispense one card, then waits for the line to drop (the card leaving, within 5 s)
/// and to come back (the next card, after 2 s at least, before 60 s: DISPENSER_JAM otherwise).
/// Without the line at boot: error 0002 DISPENSER_ERROR. Never empty, never jammed.
#[derive(Default)]
struct Dispenser {
    /// Output byte 1 of the last `32` write.
    output: u8,
    /// When the last card was dispensed.
    dispensed: Option<std::time::Instant>,
}

impl Dispenser {
    const OUT: std::time::Duration = std::time::Duration::from_millis(1000);

    fn output(&mut self, byte: u8) {
        if byte & 0x08 != 0 && self.output & 0x08 == 0 {
            log!("jvs: card dispensed");
            self.dispensed = Some(std::time::Instant::now());
        }
        self.output = byte;
    }

    fn ready(&self) -> bool {
        self.dispensed.is_none_or(|t| t.elapsed() >= Self::OUT)
    }
}

/// A ticket dispenser (`WAL_TYPEX_JVS_TICKETS`): its motor on a general purpose output, its
/// notch sensor on a switch, off at rest, on for the second half of each ticket's time while
/// the motor runs (10 tickets per second, a real dispenser's pace; the game counts the
/// pulses and stops the motor). Never empty, never jammed.
struct Tickets {
    /// The motor's output: byte index of the `32` data, bit mask.
    output: (usize, u8),
    /// The notch sensor: player 1's switch bytes 1, 2, 3 masks.
    notch: (u8, u8, u8),
    /// Since when the motor runs.
    running: Option<std::time::Instant>,
}

impl Tickets {
    const TICKET: std::time::Duration = std::time::Duration::from_millis(100);

    fn parse(spec: &str) -> Option<Tickets> {
        let (output, switch) = spec.split_once(',')?;
        let output: usize = output.trim().parse().ok().filter(|o| (1..=24).contains(o))?;
        let notch = held(switch);
        log!("jvs: ticket dispenser, motor on output {output}, notch sensor {}", switch.trim());
        Some(Tickets { output: ((output - 1) / 8, 0x80 >> ((output - 1) % 8)), notch, running: None })
    }

    /// The `32` data written: the motor started or stopped.
    fn output(&mut self, data: &[u8]) {
        let on = data.get(self.output.0).is_some_and(|b| b & self.output.1 != 0);
        match (on, self.running) {
            (true, None) => self.running = Some(std::time::Instant::now()),
            (false, Some(t)) => {
                log!("jvs: ticket motor ran {} ms", t.elapsed().as_millis());
                self.running = None;
            }
            _ => {}
        }
    }

    /// The notch sensor now: on during the second half of each ticket's time.
    fn notch(&self) -> (u8, u8, u8) {
        match self.running {
            Some(t) if (t.elapsed().as_millis() % Self::TICKET.as_millis()) >= Self::TICKET.as_millis() / 2 => self.notch,
            _ => (0, 0, 0),
        }
    }
}

/// Battle Gear 4 (`WAL_TYPEX_JVS_LAYOUT=battle-gear` / `battle-gear-pro`).
///
/// Key reader on the main board: `6F` (read), `6D` (status), `70` (UID, last byte non-zero),
/// `6A` (9 bytes), `6B` (tag data: a space, the 7-character key id, `W_OK` at 41; the id is kept
/// in `bg4-key.txt` of the data folder, created at random: 2 letters, `T`, 4 digits).
///
/// Professional cabinet: a second board (node 2) is addressed after the main one; the game
/// then runs its wide monitor & clutch mode (1360x768, 6-speed H shifter, clutch pedal).
/// Its shift up / shift down bits (main board button 2, button 3) are inverted, the H shifter
/// rows use them (top row = shift down, bottom row = shift up), the lanes are node 2's `26`
/// byte (left 0x80, right 0x40), with the shifter mechanism state (0x20 / 0x10: not in 6-speed
/// / sequential configuration), switched by node 2's `32` output (the mechanism motor).
/// Gears 1-6 = player 2's virtual b1-b6 (gear 1 left top ... gear 6 right bottom).
struct BattleGear {
    pro: bool,
    key_id: [u8; 7],
    /// Shifter mechanism in 6-speed / sequential configuration (starts in 6-speed).
    six_speed: bool,
    sequential: bool,
    /// Last node 2 output (logged on change).
    output: Vec<u8>,
}

impl BattleGear {
    fn new(pro: bool) -> Self {
        BattleGear { pro, key_id: key_id(), six_speed: true, sequential: false, output: Vec::new() }
    }

    /// (main board button 2 bit 0x01 of byte 1, button 3 bit 0x80 of byte 2, node 2 lane bits)
    /// of the H shifter gear held on player 2's stick.
    fn gear(stick: &StickState) -> (u8, u8, u8) {
        let held = |b: u32| stick.buttons & b != 0;
        let (mut b1, mut b2, mut lane) = (0, 0, 0);
        for (b, row_top, lane_bit) in [
            (button::B1, true, 0x80),
            (button::B2, false, 0x80),
            (button::B3, true, 0),
            (button::B4, false, 0),
            (button::B5, true, 0x40),
            (button::B6, false, 0x40),
        ] {
            if held(b) {
                if row_top {
                    b2 |= 0x80;
                } else {
                    b1 |= 0x01;
                }
                lane |= lane_bit;
            }
        }
        (b1, b2, lane)
    }
}

/// The key id from `bg4-key.txt` of the data folder, created when missing.
fn key_id() -> [u8; 7] {
    let path = format!("{}\\bg4-key.txt", drive::data_dir());
    if let Ok(v) = std::fs::read(&path) {
        if let Ok(id) = <[u8; 7]>::try_from(&v[..v.len().min(7)]) {
            return id;
        }
    }
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n) as u8
    };
    let id = [b'A' + next(26), b'A' + next(26), b'T', b'0' + next(10), b'0' + next(10), b'0' + next(10), b'0' + next(10)];
    let _ = std::fs::write(&path, id);
    log!("jvs: Battle Gear key id {} ({path})", String::from_utf8_lossy(&id));
    id
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// Starts the board on `WAL_TYPEX_JVS_PORT` (ports separated by commas), else `default_port`
/// (Type X `COM2`, RingEdge `mxjvs`).
pub fn init(default_port: &str) {
    let layout = std::env::var("WAL_TYPEX_JVS_LAYOUT").unwrap_or_default();
    let (haunted_museum, block_king) = (layout.trim() == "haunted-museum", layout.trim() == "block-king");
    let elevator_action = layout.trim() == "elevator-action";
    let gaia_attack = layout.trim() == "gaia-attack-4";
    let battle_gear = match layout.trim() {
        "battle-gear" => Some(BattleGear::new(false)),
        "battle-gear-pro" => Some(BattleGear::new(true)),
        _ => None,
    };
    let analog_fixed = std::env::var("WAL_TYPEX_JVS_ANALOG")
        .map(|v| v.split(',').map(|c| u16::from_str_radix(c.trim().trim_start_matches("0x"), 16).unwrap_or(0)).collect())
        .unwrap_or_default();
    let analog_inputs: Vec<_> = std::env::var("WAL_TYPEX_JVS_ANALOG_INPUTS")
        .map(|v| v.split(',').map(analog_input).collect())
        .unwrap_or_default();
    if !analog_inputs.is_empty() {
        log!("jvs: analog inputs {analog_inputs:?}");
    }
    *BOARD.lock().unwrap() = Some(Board {
        map: ButtonMap::new(NATIVES, DEFAULT_MAP),
        coins: [0; 4],
        coin_held: [false; 4],
        coin_slots: 2,
        haunted_museum,
        gaia_attack,
        block_king,
        elevator_action,
        cyber_diver: std::env::var("WAL_TYPEX_JVS_LAYOUT").is_ok_and(|v| v == "cyber-diver"),
        happy_playground: layout == "happy-playground",
        levers: layout.trim() == "levers",
        h_shifter: layout.trim() == "h-shifter",
        dispenser: (layout.trim() == "k-on").then(Dispenser::default),
        tickets: std::env::var("WAL_TYPEX_JVS_TICKETS").ok().and_then(|v| Tickets::parse(&v)),
        wontertainment: layout.trim() == "wontertainment",
        won_output: 0,
        won_dispensed: None,
        analog_fixed,
        analog_inputs,
        touch: std::env::var("WAL_TYPEX_JVS_TOUCH").ok().and_then(|v| {
            let c: Vec<f32> = v.split(',').filter_map(|c| c.trim().parse().ok()).collect();
            let c = <[f32; 8]>::try_from(c).ok();
            match c {
                Some(c) => log!("jvs: touch screen, raw corners {c:?}"),
                None => log!("jvs: bad WAL_TYPEX_JVS_TOUCH {v:?} (8 numbers)"),
            }
            c
        }),
        held: std::env::var("WAL_TYPEX_JVS_HELD").map(|v| held(&v)).unwrap_or_default(),
        outputs: std::env::var("WAL_TYPEX_JVS_OUTPUTS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0),
        rotary: std::env::var("WAL_TYPEX_JVS_ROTARY").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0),
        identifier: std::env::var("WAL_TYPEX_JVS_IDENTIFIER")
            .map(|v| [v.trim().as_bytes(), b"\0"].concat())
            .unwrap_or_else(|_| IDENTIFIER.to_vec()),
        switches: std::env::var("WAL_TYPEX_JVS_SWITCHES")
            .ok()
            .and_then(|v| {
                let (p, s) = v.trim().split_once('x')?;
                Some((p.parse().ok()?, s.parse().ok()?))
            })
            .unwrap_or((2, 16)),
        pulse: std::env::var("WAL_TYPEX_JVS_PULSE").ok().and_then(|v| pulse(&v)),
        battle_gear,
        addresses: 0,
    });
    let port = std::env::var("WAL_TYPEX_JVS_PORT").unwrap_or_else(|_| default_port.into());
    let mut modules = vec![unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize];
    if let Ok(dlls) = std::env::var("WAL_TYPEX_JVS_DLLS") {
        for name in dlls.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let Ok(path) = std::ffi::CString::new(name) else { continue };
            let module = unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryA(path.as_ptr().cast()) };
            if module.is_null() {
                log!("jvs: cannot load I/O module {name}");
            } else {
                modules.push(module as usize);
                log!("jvs: serial hooks in I/O module {name}");
                if std::env::var("WAL_TYPEX_XIO_CALIBRATION").is_ok_and(|v| v == "1") {
                    unsafe { install_xio_calibration(module); }
                }
            }
        }
    }
    // a list: the same board on each (a RingEdge game on mxjvs, its test menu on COM4)
    for port in port.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        serial::install_in(port, process, &modules);
    }
    log!("jvs: I/O board on {port}");
}

static XIO_CALIBRATION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// XIO receives four min/max pairs at offset 0x30 in its analog parameter block.
/// A fresh Cyber Diver dump loads zero pairs without testmode.exe to calibrate them.
#[cfg(target_arch = "x86")]
unsafe fn install_xio_calibration(module: windows_sys::Win32::Foundation::HMODULE) {
    use windows_sys::Win32::System::{LibraryLoader::GetProcAddress, Memory::{VirtualProtect, PAGE_READWRITE}};
    let Some(get) = (unsafe { GetProcAddress(module, c"?GetXIO@@YAPAVIXioManager@@XZ".as_ptr().cast()) }) else { return };
    let get: unsafe extern "C" fn() -> *mut usize = unsafe { std::mem::transmute(get) };
    let object = unsafe { get() };
    if object.is_null() { return; }
    let table = unsafe { *object } as *mut usize;
    let slot = unsafe { table.add(0x28 / 4) };
    let mut old = 0;
    if unsafe { VirtualProtect(slot.cast(), 4, PAGE_READWRITE, &mut old) } == 0 { return; }
    XIO_CALIBRATION.store(unsafe { *slot }, Ordering::Relaxed);
    unsafe {
        *slot = xio_calibration as *const () as usize;
        VirtualProtect(slot.cast(), 4, old, &mut old);
    }
    log!("jvs: XIO fallback calibration installed");
}

#[cfg(not(target_arch = "x86"))]
unsafe fn install_xio_calibration(_: windows_sys::Win32::Foundation::HMODULE) {}

#[cfg(target_arch = "x86")]
unsafe extern "thiscall" fn xio_calibration(object: *mut usize, parameters: *const u8) -> i32 {
    let mut values = [0u8; 64];
    // The original setter only reads these initialized calibration words.
    unsafe { std::ptr::copy_nonoverlapping(parameters.add(0x30), values.as_mut_ptr().add(0x30), 16); }
    for pair in values[0x30..0x40].chunks_exact_mut(4) {
        if pair[..2] == pair[2..] {
            // Symmetric around 0x8000; retain every valid saved calibration pair.
            pair.copy_from_slice(&[1, 0, 0xff, 0xff]);
        }
    }
    let original: unsafe extern "thiscall" fn(*mut usize, *const u8) -> i32 =
        unsafe { std::mem::transmute(XIO_CALIBRATION.load(Ordering::Relaxed)) };
    unsafe { original(object, values.as_ptr()) }
}

/// One `WAL_TYPEX_JVS_ANALOG_INPUTS` entry: `accel`, `-lx` (inverted), `+ry` (positive half),
/// empty for none; player 1's axis, `p<n>:` before it another player's (`p2:-ly`), `*scale`
/// after it scaled around the center, `@center` (hex) around another center. Returns the
/// player (0 based), the input, its scale and center.
fn analog_input(entry: &str) -> Option<(usize, AnalogInput, f32, u16)> {
    let entry = entry.trim();
    let (player, entry) = match entry.split_once(':') {
        Some((p, rest)) if p.len() == 2 && p.starts_with('p') => match p[1..].parse::<usize>() {
            Ok(n @ 1..=4) => (n - 1, rest.trim()),
            _ => {
                log!("jvs: unknown analog input player {p:?}");
                return None;
            }
        },
        _ => (0, entry),
    };
    let (entry, center) = match entry.split_once('@') {
        Some((e, c)) => match u16::from_str_radix(c.trim(), 16) {
            Ok(c) => (e.trim(), c),
            Err(_) => {
                log!("jvs: bad analog center {c:?}");
                (e.trim(), 0x8000)
            }
        },
        None => (entry, 0x8000),
    };
    let (entry, scale) = match entry.split_once('*') {
        Some((e, f)) => match f.trim().parse::<f32>() {
            Ok(f) => (e.trim(), f),
            Err(_) => {
                log!("jvs: bad analog scale {f:?}");
                (e.trim(), 1.0)
            }
        },
        None => (entry, 1.0),
    };
    let (name, kind): (&str, fn(Axis) -> AnalogInput) = match entry.as_bytes().first() {
        Some(b'-') => (&entry[1..], AnalogInput::Inverted),
        Some(b'+') => (&entry[1..], AnalogInput::Positive),
        _ => (entry, AnalogInput::Axis),
    };
    let axis = Axis::from_name(name);
    if axis.is_none() && !name.is_empty() {
        log!("jvs: unknown analog input {entry:?}");
    }
    axis.map(|a| (player, kind(a), scale, center))
}

/// `WAL_TYPEX_JVS_PULSE=<button>,...@<ms>`: the switch masks of the buttons and the half period.
fn pulse(spec: &str) -> Option<([u8; 3], std::time::Duration)> {
    let (buttons, ms) = spec.split_once('@')?;
    let mut masks = [0u8; 3];
    for n in buttons.split(',').filter_map(|b| b.trim().parse::<u8>().ok()) {
        match n {
            1 => masks[0] |= 0x02,
            2 => masks[0] |= 0x01,
            3..=8 => masks[1] |= 0x80 >> (n - 3),
            9..=16 => masks[2] |= 0x80 >> (n - 9),
            _ => {}
        }
    }
    let half = std::time::Duration::from_millis(ms.trim().parse().ok()?);
    log!("jvs: pulsed switches {masks:02X?}, {half:?} on / off");
    Some((masks, half))
}

/// Analog channel value, left-justified 16 bits (10 significant bits, as the features say).
fn analog(board: &Board, sticks: &[StickState; 4], channel: usize) -> u16 {
    let Some((player, input, scale, center)) = board.analog_inputs.get(channel).copied().flatten() else {
        return board.analog_fixed.get(channel).copied().unwrap_or(0);
    };
    let stick = &sticks[player];
    let value = match input {
        AnalogInput::Axis(axis) => stick.axis_u16(axis),
        AnalogInput::Inverted(axis) => !stick.axis_u16(axis),
        AnalogInput::Positive(axis) => (stick.axis(axis).max(0) as u32 * 2) as u16,
    };
    let value = if scale == 1.0 && center == 0x8000 {
        value
    } else {
        (center as f32 + (value as f32 - 32768.0) * scale).clamp(0.0, 65535.0) as u16
    };
    value & 0xFFC0
}

/// The touch panel's raw position (x, y) of player 1's pointer: its place in the picture,
/// bilinear between the panel's raw corners.
fn touch(c: &[f32; 8], stick: &StickState) -> (u16, u16) {
    let frac = |v: i16| (v as f32 + 32768.0) / 65535.0;
    let (u, v) = crate::screenshot::to_picture(frac(stick.axis(Axis::RightX)), frac(stick.axis(Axis::RightY)));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let at = |k: usize| lerp(lerp(c[k], c[k + 2], u), lerp(c[k + 4], c[k + 6], u), v);
    (at(0).round().clamp(0.0, 65535.0) as u16, at(1).round().clamp(0.0, 65535.0) as u16)
}

/// `WAL_TYPEX_JVS_HELD`'s buttons as (switch byte 1, 2, 3) bits.
fn held(v: &str) -> (u8, u8, u8) {
    let mut bits = (0u8, 0u8, 0u8);
    for name in v.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        match name.strip_prefix("btn").and_then(|n| n.parse::<u8>().ok()) {
            Some(1) => bits.0 |= 0x02,
            Some(2) => bits.0 |= 0x01,
            Some(n @ 3..=8) => bits.1 |= 0x80 >> (n - 3),
            Some(n @ 9..=16) => bits.2 |= 0x80 >> (n - 9),
            _ => log!("jvs: bad WAL_TYPEX_JVS_HELD switch {name:?} (btn1-btn16)"),
        }
    }
    bits
}

/// (switch byte 1, switch byte 2, test, coin, switch byte 3) of a player, and its
/// [`Native::P2`] bits.
fn player(map: &ButtonMap<Native>, stick: &StickState) -> ((u8, u8, bool, bool, u8), (u8, u8)) {
    let (mut b1, mut b2, mut test, mut coin, mut b3) = (0u8, 0u8, false, false, 0u8);
    let mut other = (0u8, 0u8);
    map.for_each_pressed(stick, |n| match n {
        Native::Start => b1 |= 0x80,
        Native::Service => b1 |= 0x40,
        Native::Up => b1 |= 0x20,
        Native::Down => b1 |= 0x10,
        Native::Left => b1 |= 0x08,
        Native::Right => b1 |= 0x04,
        Native::Btn(1) => b1 |= 0x02,
        Native::Btn(2) => b1 |= 0x01,
        Native::Btn(n @ 3..=8) => b2 |= 0x80 >> (n - 3),
        Native::Btn(n @ 9..=16) => b3 |= 0x80 >> (n - 9),
        Native::Btn(_) => {}
        Native::Test => test = true,
        Native::Coin => coin = true,
        Native::P2(x1, x2) => {
            other.0 |= x1;
            other.1 |= x2;
        }
    });
    ((b1, b2, test, coin, b3), other)
}

fn process(packet: &[u8]) -> Vec<u8> {
    let Some(req) = parse(packet) else { return Vec::new() };
    let cmds = &req.commands[..];
    let mut guard = BOARD.lock().unwrap();
    let Some(board) = guard.as_mut() else { return Vec::new() };
    let pro = board.battle_gear.as_ref().is_some_and(|b| b.pro);
    // only the broadcast and our own node (1; 2: Battle Gear's second board)
    if req.node != 0xFF && req.node > if pro { 0x02 } else { 0x01 } {
        return Vec::new();
    }
    // bus reset: no reply, the board is unaddressed again (sense line). K-On! resets the bus
    // once more after its first polls and only assigns the address when the sense line says
    // so (JVS_BOARD_NONE otherwise).
    if cmds.first() == Some(&0xF0) {
        serial::READY.store(false, Ordering::Relaxed);
        board.addresses = 0;
        return Vec::new();
    }
    if req.node == 0x02 {
        return node2(board, cmds);
    }
    // Won!Tertainment Music Channel's two I/O threads sometimes write its input poll's header
    // over its output packet (`13 32 02 a b 33 06 ...`): a checksum error, which a board
    // reports (status 3) and the game resends the same bytes, then stops (error 903, "JVS
    // error"). The packet is answered as it parses; its outputs a b, still before `33 06`, hold
    // the card dispenser's motor.
    if board.wontertainment && crate::jvs::bad_checksum(packet) {
        if let Some(p) = packet.windows(2).skip(5).position(|w| w == [0x33, 0x06]).map(|p| p + 5) {
            won_output(board, packet[p - 2]);
        }
    }

    // input state (coins counted on release)
    let sticks = [0, 1, 2, 3].map(crate::input);
    let both: Vec<_> = sticks.iter().map(|s| player(&board.map, s)).collect();
    let mut players: Vec<_> = both.iter().map(|b| b.0).collect();
    for (_, (x1, x2)) in &both {
        players[1].0 |= x1;
        players[1].1 |= x2;
    }
    for (n, p) in players.iter().enumerate() {
        if board.coin_held[n] && !p.3 {
            let slot = n.min(board.coin_slots.max(1) - 1);
            board.coins[slot] = board.coins[slot].saturating_add(1);
        }
        board.coin_held[n] = p.3;
    }
    players[0].0 |= board.held.0;
    players[0].1 |= board.held.1;
    players[0].4 |= board.held.2;
    if board.levers {
        levers(&mut players, &sticks[0]);
    }
    if board.h_shifter {
        players[1].0 = (players[1].0 & !0x3C) | crate::hshifter::gate(crate::hshifter::gear(sticks[0].buttons));
    }
    if let Some(hook) = SWITCHES_HOOK.get() {
        hook(&mut players);
    }
    if board.happy_playground {
        players[0].4 |= 0x40;
    }
    if let Some(d) = &board.dispenser {
        // the dispenser's lines replace buttons 3-5 (a held button would read as a jam)
        players[0].1 = (players[0].1 & !0xE0) | if d.ready() { 0x80 } else { 0 };
    }
    if let Some(t) = &board.tickets {
        let notch = t.notch();
        players[0].0 |= notch.0;
        players[0].1 |= notch.1;
        players[0].4 |= notch.2;
    }
    let test = players.iter().any(|p| p.2);
    if let Some((masks, half)) = board.pulse {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let phase = START.get_or_init(std::time::Instant::now).elapsed().as_millis() / half.as_millis().max(1);
        if phase % 2 == 1 {
            for p in &mut players {
                p.0 &= !masks[0];
                p.1 &= !masks[1];
                p.4 &= !masks[2];
            }
        }
    }

    let mut out = Encoder::new();
    out.push(0x01); // report of the first command
    let mut i = 0;
    let mut multi = false;
    while i < cmds.len() {
        let arg = |k: usize| cmds.get(i + k).copied().unwrap_or(0);
        let rep = |v: &mut Vec<u8>| {
            if multi {
                v.insert(0, 0x01)
            }
        };
        let (len, mut bytes): (usize, Vec<u8>) = match cmds[i] {
            0xF1 => {
                // with 2 boards, the sense line says "all addressed" after the second one
                board.addresses = board.addresses.saturating_add(1);
                if board.addresses >= if pro { 2 } else { 1 } {
                    serial::READY.store(true, Ordering::Relaxed);
                }
                (2, vec![])
            }
            0x10 => (1, board.identifier.clone()),
            0x11 => (1, vec![COMMAND_REVISION]),
            0x12 => (1, vec![JVS_VERSION]),
            0x13 => (1, vec![COMM_VERSION]),
            // features, Taito stick: 2 players x 16 switches (`WAL_TYPEX_JVS_SWITCHES`), 2 coin
            // slots (+ 8 analog channels of 10 bits for the games reading axes)
            0x14 => {
                let mut f = vec![0x01, board.switches.0, board.switches.1, 0x00, 0x02, 0x02, 0x00, 0x00];
                if !board.analog_inputs.is_empty() {
                    f.extend_from_slice(&[0x03, 0x08, 0x0A, 0x00]);
                }
                if board.rotary > 0 {
                    f.extend_from_slice(&[0x04, board.rotary, 0x00, 0x00]);
                }
                if board.outputs > 0 {
                    // general purpose outputs (lamps, gun recoil): `32` writes
                    f.extend_from_slice(&[0x12, board.outputs, 0x00, 0x00]);
                }
                // the end code; padded to a whole entry when nothing follows (a game asking more
                // in the packet, CHUNITHM's `14 11 12 13`, reads their answers right after it)
                if i + 1 < cmds.len() {
                    f.push(0x00);
                } else {
                    f.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
                }
                (1, f)
            }
            0x15 => (cmds[i..].iter().position(|b| *b == 0).map_or(cmds.len() - i, |p| p + 1), vec![0x01, 0x01, 0x05]),
            0x20 if board.wontertainment => {
                let p1 = players[0];
                let (left, right) = (p1.0 & 0x02 != 0, p1.0 & 0x01 != 0);
                let (select, enter) = (p1.1 & 0x20 != 0, p1.1 & 0x10 != 0);
                let system = if select { 0x80 } else { 0 };
                // after the motor starts (dispense): the card leaves the gate (its sensor off
                // for 1.5 s, the next card fed back; the game checks it 0.5 s after the start and
                // polls slowly), lies in the exit tray (CARD OUT 0.6-2.5 s) and is taken
                let since = board.won_dispensed.map(|t| t.elapsed().as_millis());
                let gate = !since.is_some_and(|ms| ms < 1500);
                let out = since.is_some_and(|ms| (600..2500).contains(&ms));
                let b1 = if gate { 0x10 } else { 0 } | if enter { 0x40 } else { 0 };
                let b2 = if left { 0x80 } else { 0 } | if right { 0x40 } else { 0 } | if p1.0 & 0x40 != 0 { 0x08 } else { 0 } | if out { 0x04 } else { 0 } | if test { 0x01 } else { 0 };
                let bytes = [b1, b2, 0, 0];
                (3, std::iter::once(system).chain((0..arg(1) as usize * arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0))).collect())
            }
            0x20 if (arg(1), arg(2)) == (2, 2) || arg(1) == 0 => {
                // Taito stick: system byte + 2 bytes per player for both players
                let p1 = players[0];
                let p2 = players[1];
                (3, vec![if test { 0x80 } else { 0 }, p1.0, p1.1, p2.0, p2.1])
            }
            0x20 if board.block_king && arg(1) == 1 => {
                let (p1, p2) = (players[0], players[1]);
                let mut b0 = (p1.0 | p2.0) & 0x40;
                if p1.0 & 0x80 != 0 {
                    b0 |= 0x20;
                }
                if p2.0 & 0x80 != 0 {
                    b0 |= 0x10;
                }
                if p1.0 & 0x01 != 0 {
                    b0 |= 0x08;
                }
                let mut b1 = 0;
                if p1.1 & 0x40 != 0 {
                    b1 |= 0x08;
                }
                if p1.1 & 0x80 != 0 {
                    b1 |= 0x04;
                }
                let bytes = [b0, b1, 0];
                (3, std::iter::once(if test { 0x80 } else { 0 }).chain((0..arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0))).collect())
            }
            0x20 if board.elevator_action && arg(1) == 1 => {
                // Elevator Action Death Parade: one 4-byte block for the whole cabinet. Each
                // gun's button is its own bit (player 1's native button 1: 0x02, player 2's:
                // 0x01); the elevator panel (up 0x20, action 0x08 = left, down 0x10), service,
                // SELECT / ENTER (buttons 7 / 8) from either player
                let (p1, p2) = (players[0], players[1]);
                let b0 = ((p1.0 | p2.0) & 0xFC) | (p1.0 & 0x02) | ((p2.0 & 0x02) >> 1);
                let bytes = [b0, p1.1 | p2.1, p1.4 | p2.4, 0];
                (3, std::iter::once(if test { 0x80 } else { 0 }).chain((0..arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0))).collect())
            }
            0x20 if board.gaia_attack && arg(1) == 1 => {
                let start = |n: usize| players[n].0 & 0x80 != 0;
                let b0 = if start(0) { 0x20 } else { 0 } | if start(1) { 0x10 } else { 0 };
                let mut b2 = if start(2) { 0x80 } else { 0 } | if start(3) { 0x40 } else { 0 };
                if players.iter().any(|p| p.0 & 0x40 != 0) {
                    b2 |= 0x08;
                }
                let bytes = [b0, 0, b2];
                (3, std::iter::once(if test { 0x80 } else { 0 }).chain((0..arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0))).collect())
            }
            0x20 if board.haunted_museum && arg(1) == 1 => {
                let (p1, p2) = (players[0], players[1]);
                let mut b0 = 0;
                if p1.0 & 0x80 != 0 {
                    b0 |= 0x20;
                }
                if p2.0 & 0x80 != 0 {
                    b0 |= 0x10;
                }
                let mut b2 = 0xC0;
                if (p1.0 | p2.0) & 0x40 != 0 {
                    b2 |= 0x08;
                }
                // actions (button 2), active low
                if p1.0 & 0x01 != 0 {
                    b2 &= !0x80;
                }
                if p2.0 & 0x01 != 0 {
                    b2 &= !0x40;
                }
                let bytes = [b0, 0, b2];
                (3, std::iter::once(if test { 0x80 } else { 0 }).chain((0..arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0))).collect())
            }
            0x20 if pro && arg(1) == 1 => {
                // Battle Gear professional: H shifter rows on shift up / shift down, both
                // inverted
                let (g1, g2, _) = BattleGear::gear(&sticks[1]);
                let p1 = players[0];
                let bytes = [(p1.0 | g1) ^ 0x01, (p1.1 | g2) ^ 0x80, p1.4];
                (3, std::iter::once(if test { 0x80 } else { 0 }).chain((0..arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0))).collect())
            }
            0x20 => {
                // other layouts (Gaia Attack 4: 1 player x 3 bytes), generic
                // reply: the 3 switch bytes of each player, padded with zeros
                let mut v = vec![if test { 0x80 } else { 0 }];
                for p in players.iter().take(arg(1).min(2) as usize) {
                    let bytes = [p.0, p.1 ^ if board.cyber_diver { 0x0c } else { 0 }, p.4];
                    v.extend((0..arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0)));
                }
                (3, v)
            }
            0x21 => {
                let slots = arg(1) as usize;
                board.coin_slots = slots;
                let mut v = Vec::new();
                for s in 0..slots {
                    let c = board.coins.get(s).copied().unwrap_or(0);
                    v.extend_from_slice(&c.to_be_bytes());
                }
                (2, v)
            }
            0x22 => {
                // analog channels: fixed values (the Haunted Museum games read their volume
                // knob on channel 0, 0: silent) or the players' axes
                (2, (0..arg(1).max(1) as usize).flat_map(|c| analog(board, &sticks, c).to_be_bytes()).collect())
            }
            0x26 => (2, vec![0; arg(1) as usize]),
            0x2E => (2, vec![0; 4]),
            0x2F => (1, vec![0; 5]),
            0x30 | 0x31 => {
                let slot = arg(1).saturating_sub(1) as usize;
                let count = u16::from_be_bytes([arg(2), arg(3)]);
                if let Some(c) = board.coins.get_mut(slot) {
                    *c = if cmds[i] == 0x30 { c.saturating_sub(count) } else { c.saturating_add(count) };
                }
                (4, vec![])
            }
            0x32 => {
                if let Some(t) = &mut board.tickets {
                    t.output(cmds.get(i + 2..(i + 2 + arg(1) as usize).min(cmds.len())).unwrap_or(&[]));
                }
                if let Some(d) = &mut board.dispenser {
                    d.output(arg(2));
                }
                if board.wontertainment {
                    won_output(board, arg(2));
                }
                (arg(1) as usize + 2, vec![])
            }
            0x33 => (arg(1) as usize * 2 + 2, vec![]),
            0x34 => (arg(1) as usize + 2, vec![]),
            0x36 => (4, vec![]),
            0x37 => (3, vec![]),
            // Taito specific
            0x01 => (2, vec![0x01, 0x01]),
            0x03 => (2, vec![0x01]),
            0x04 => (1, vec![]),
            0x05 => (3, vec![]),
            // watchdog kick (New Super Mario Bros. Wii Coin World polls it with `01 01`)
            0x08 => (1, vec![0x00]),
            // rotary channels: 2 bytes each
            0x23 => (2, vec![0; 2 * arg(1) as usize]),
            // Screen position: each requested channel is a pair of 16-bit coordinates.
            // Cyber Diver's XIO parser consumes report+4 bytes even without a touch device;
            // omitting them rejects the complete combined switch/coin/analog report.
            0x25 => {
                let mut v = vec![0; arg(1) as usize * 4];
                if let (Some(c), true) = (board.touch, v.len() >= 4) {
                    let (x, y) = touch(&c, &sticks[0]);
                    v[..4].copy_from_slice(&[(x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8]);
                }
                (2, v)
            }
            0x65 => (2, vec![0xA0]),
            // Battle Gear key reader (these replies carry their own report byte)
            0x6F if board.battle_gear.is_some() => (1, vec![0x01]),
            0x6D if board.battle_gear.is_some() => (1, vec![0x01, 0x00]),
            0x70 if board.battle_gear.is_some() => (1, vec![0x01, 0, 0, 0, 0, 0, 0, 0, 0x09]),
            0x6A if board.battle_gear.is_some() => (9, vec![0x01]),
            0x6B if board.battle_gear.is_some() => {
                let mut v = vec![0xFF; 0x2D];
                v[0] = 0x01;
                v[1] = b' ';
                if let Some(bg) = &board.battle_gear {
                    v[2..9].copy_from_slice(&bg.key_id);
                }
                v[41..45].copy_from_slice(b"W_OK");
                (1, v)
            }
            0x66 if pro => (3, vec![0x01]),
            // Gaia Attack 4 polls `67 xx` in every packet: unknown, acknowledged so the
            // commands after it (coins, analogs) are answered; Elevator Action Death Parade
            // polls `7D 00` the same way
            0x67 | 0x7D => (2, vec![]),
            other => {
                log!("jvs: unknown command {other:#04x}");
                (cmds.len() - i, vec![])
            }
        };
        // commands with outputs/acks: data commands get a report
        // byte when they are not the first one
        if matches!(cmds[i], 0x08 | 0x11..=0x14 | 0x20..=0x22 | 0x25 | 0x26 | 0x2E | 0x30..=0x37 | 0x65 | 0x67 | 0x7D) {
            rep(&mut bytes);
        }
        out.extend(&bytes);
        i += len.max(1);
        multi = true;
    }
    out.finish()
}

/// Won!Tertainment Music Channel's output byte 1: the card dispenser's motor (bit 0x10) starts.
fn won_output(board: &mut Board, out: u8) {
    if out & 0x10 != 0 && board.won_output & 0x10 == 0 {
        log!("jvs: card dispensed");
        board.won_dispensed = Some(std::time::Instant::now());
    }
    board.won_output = out;
}

/// Battle Gear 4's professional cabinet board (node 2): no inputs but the H shifter lanes and
/// mechanism state (`26`), the mechanism motor (`32`); it reports the coin counters too (the
/// game reads the coins from it in this mode).
fn node2(board: &mut Board, cmds: &[u8]) -> Vec<u8> {
    let coins = board.coins;
    let Some(bg) = board.battle_gear.as_mut() else { return Vec::new() };
    let gear = BattleGear::gear(&crate::input(1));
    let mut coin_change = None;
    let mut out = Encoder::new();
    out.push(0x01);
    let mut i = 0;
    let mut multi = false;
    while i < cmds.len() {
        let arg = |k: usize| cmds.get(i + k).copied().unwrap_or(0);
        let (len, mut bytes): (usize, Vec<u8>) = match cmds[i] {
            0x10 => (1, IDENTIFIER.to_vec()),
            0x11 => (1, vec![COMMAND_REVISION]),
            0x12 => (1, vec![JVS_VERSION]),
            0x13 => (1, vec![COMM_VERSION]),
            0x14 => (1, vec![0x01, 0x02, 0x10, 0x00, 0x02, 0x02, 0x00, 0x00, 0x03, 0x08, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00]),
            0x15 => (cmds[i..].iter().position(|b| *b == 0).map_or(cmds.len() - i, |p| p + 1), vec![0x01, 0x01, 0x05]),
            0x20 => {
                let bytes = [gear.0 ^ 0x01, gear.1 ^ 0x80];
                (3, std::iter::once(0).chain((0..(arg(1) as usize * arg(2) as usize)).map(|k| bytes.get(k).copied().unwrap_or(0))).collect())
            }
            0x21 => (2, (0..arg(1) as usize).flat_map(|s| coins.get(s).copied().unwrap_or(0).to_be_bytes()).collect()),
            0x22 => (2, vec![0; arg(1).max(1) as usize * 2]),
            0x30 | 0x31 => {
                coin_change = Some((arg(1).saturating_sub(1) as usize, u16::from_be_bytes([arg(2), arg(3)]), cmds[i] == 0x30));
                (4, vec![])
            }
            0x26 => {
                let mut v = vec![0; arg(1).max(1) as usize];
                v[0] = gear.2 | if bg.six_speed { 0 } else { 0x20 } | if bg.sequential { 0 } else { 0x10 };
                (2, v)
            }
            0x32 => {
                // the mechanism motor switches between the 6-speed and sequential configurations:
                // once per motor start (the game drives it until the sensors change)
                let output = cmds[i..(i + 2 + arg(1) as usize).min(cmds.len())].to_vec();
                if arg(1) > 0 && arg(2) > 0 && bg.output.get(2).is_none_or(|v| *v == 0) {
                    bg.six_speed = !bg.six_speed;
                    bg.sequential = !bg.sequential;
                    log!("jvs: Battle Gear shifter {}", if bg.six_speed { "6-speed" } else { "sequential" });
                }
                if output != bg.output {
                    log!("jvs: node 2: output {output:02X?}");
                    bg.output = output;
                }
                (arg(1) as usize + 2, vec![])
            }
            0x66 => (3, vec![0x01]),
            0x00 | 0x02 | 0x40 | 0xFF => (1, vec![]),
            other => {
                log!("jvs: node 2: unknown command {other:#04x}");
                (cmds.len() - i, vec![])
            }
        };
        if multi && matches!(cmds[i], 0x11..=0x14 | 0x20..=0x22 | 0x26 | 0x30..=0x32) {
            bytes.insert(0, 0x01);
        }
        out.extend(&bytes);
        i += len.max(1);
        multi = true;
    }
    if let Some((slot, count, decrease)) = coin_change {
        if let Some(c) = board.coins.get_mut(slot) {
            *c = if decrease { c.saturating_sub(count) } else { c.saturating_add(count) };
        }
    }
    out.finish()
}
