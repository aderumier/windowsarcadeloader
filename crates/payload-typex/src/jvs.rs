//! JVS I/O board for Taito Type X: Taito stick mode (2 players, 16 switches), JVS version 0x30.
//!
//! Replies: one report byte for the packet, then each command's data; the commands after
//! the first add their own report byte.
//!
//! Switch byte 1: start 0x80, service 0x40, up 0x20, down 0x10, left 0x08, right 0x04,
//! button 1 0x02, button 2 0x01. Switch byte 2: buttons 3-8 0x80/0x40/0x20/0x10/0x08/0x04.
//! System byte: test 0x80. Coins count up when the coin input is released.
//!
//! Analog channels: `WAL_TYPEX_JVS_ANALOG` (fixed values, e.g. a volume knob) and
//! `WAL_TYPEX_JVS_ANALOG_INPUTS` (player 1's axes, e.g. the pedals of driving games).
//!
//! `WAL_TYPEX_JVS_LAYOUT=battle-gear` / `battle-gear-pro`: Battle Gear 4's key reader (Taito
//! commands `6A`-`70`); `-pro` adds the professional cabinet's second board (node 2, its
//! presence makes the game use the wide monitor & clutch mode) and the H shifter (see
//! [`BattleGear`]).

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use wal_payload_common::jvs::{Encoder, parse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{drive, log, serial};
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
    coins: [u16; 2],
    coin_held: [bool; 2],
    /// `WAL_TYPEX_JVS_LAYOUT=haunted-museum`: the gun cabinets' switch wiring (Haunted Museum
    /// 1/2) for `20 01 03` (1 player, 3 bytes): P1 start on up 0x20, P2 start on down 0x10;
    /// third byte: service 0x08, P2/P1 action 0x40/0x80 active low (coins: the coin counter). With the generic reply the actions read as held and up/down as
    /// both starts.
    haunted_museum: bool,
    /// `WAL_TYPEX_JVS_LAYOUT=block-king`: Block King Ball Shooter's switches for `20 01 03`
    /// (found with its switch test): first byte service 0x40, left (P1) start 0x20, right (P2)
    /// start 0x10, cannon 0x08 (button 2); second byte SELECT 0x08 (button 4), ENTER 0x04
    /// (button 3). The standard start bit 0x80 is not read.
    block_king: bool,
    /// `WAL_TYPEX_JVS_ANALOG=v0,v1,...` (hex): fixed analog channel values (default 0).
    analog_fixed: Vec<u16>,
    /// `WAL_TYPEX_JVS_ANALOG_INPUTS=<axis>,...`: player 1's virtual axis read on each
    /// channel (`-axis` inverted; empty: the fixed value). When set, the features report 8
    /// analog channels: games poll `22` only for the channels the board declares (Valve
    /// Limit R: gas on channel 1, brake on channel 2).
    analog_inputs: Vec<Option<AnalogInput>>,
    battle_gear: Option<BattleGear>,
    /// Addresses assigned since the last bus reset (2 boards with `battle-gear-pro`).
    addresses: u8,
}

/// Analog channel source: player 1's axis, inverted (`-axis`) or its positive half only
/// (`+axis`: 0 at the center and below, a clutch on a stick axis).
#[derive(Clone, Copy, Debug)]
enum AnalogInput {
    Axis(Axis),
    Inverted(Axis),
    Positive(Axis),
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

pub(crate) fn init() {
    let layout = std::env::var("WAL_TYPEX_JVS_LAYOUT").unwrap_or_default();
    let (haunted_museum, block_king) = (layout.trim() == "haunted-museum", layout.trim() == "block-king");
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
        coins: [0; 2],
        coin_held: [false; 2],
        haunted_museum,
        block_king,
        analog_fixed,
        analog_inputs,
        battle_gear,
        addresses: 0,
    });
    let port = std::env::var("WAL_TYPEX_JVS_PORT").unwrap_or_else(|_| "COM2".into());
    serial::install(&port, process);
    log!("jvs: I/O board on {port}");
}

/// One `WAL_TYPEX_JVS_ANALOG_INPUTS` entry: `accel`, `-lx` (inverted), `+ry` (positive half),
/// empty for none.
fn analog_input(entry: &str) -> Option<AnalogInput> {
    let entry = entry.trim();
    let (name, kind): (&str, fn(Axis) -> AnalogInput) = match entry.as_bytes().first() {
        Some(b'-') => (&entry[1..], AnalogInput::Inverted),
        Some(b'+') => (&entry[1..], AnalogInput::Positive),
        _ => (entry, AnalogInput::Axis),
    };
    let axis = Axis::from_name(name);
    if axis.is_none() && !name.is_empty() {
        log!("jvs: unknown analog input {entry:?}");
    }
    axis.map(kind)
}

/// Analog channel value, left-justified 16 bits (10 significant bits, as the features say).
fn analog(board: &Board, stick: &StickState, channel: usize) -> u16 {
    match board.analog_inputs.get(channel).copied().flatten() {
        Some(AnalogInput::Axis(axis)) => stick.axis_u16(axis) & 0xFFC0,
        Some(AnalogInput::Inverted(axis)) => !stick.axis_u16(axis) & 0xFFC0,
        Some(AnalogInput::Positive(axis)) => ((stick.axis(axis).max(0) as u32 * 2) as u16) & 0xFFC0,
        None => board.analog_fixed.get(channel).copied().unwrap_or(0),
    }
}

/// (switch byte 1, switch byte 2, test, coin, switch byte 3) of a player.
fn player(map: &ButtonMap<Native>, stick: &StickState) -> (u8, u8, bool, bool, u8) {
    let (mut b1, mut b2, mut test, mut coin, mut b3) = (0u8, 0u8, false, false, 0u8);
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
    });
    (b1, b2, test, coin, b3)
}

fn process(packet: &[u8]) -> Vec<u8> {
    let Some(req) = parse(packet) else { return Vec::new() };
    let cmds = req.commands;
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

    // input state (coins counted on release)
    let sticks = [wal_payload_common::input(0), wal_payload_common::input(1)];
    let players: Vec<_> = sticks.iter().map(|s| player(&board.map, s)).collect();
    for (slot, p) in players.iter().enumerate() {
        if board.coin_held[slot] && !p.3 {
            board.coins[slot] = board.coins[slot].saturating_add(1);
        }
        board.coin_held[slot] = p.3;
    }
    let test = players.iter().any(|p| p.2);

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
            0x10 => (1, IDENTIFIER.to_vec()),
            0x11 => (1, vec![COMMAND_REVISION]),
            0x12 => (1, vec![JVS_VERSION]),
            0x13 => (1, vec![COMM_VERSION]),
            // features, Taito stick: 2 players x 16 switches, 2 coin slots (+ 8 analog
            // channels of 10 bits for the games reading axes)
            0x14 if !board.analog_inputs.is_empty() => {
                (1, vec![0x01, 0x02, 0x10, 0x00, 0x02, 0x02, 0x00, 0x00, 0x03, 0x08, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00])
            }
            0x14 => (1, vec![0x01, 0x02, 0x10, 0x00, 0x02, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
            0x15 => (cmds[i..].iter().position(|b| *b == 0).map_or(cmds.len() - i, |p| p + 1), vec![0x01, 0x01, 0x05]),
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
                    let bytes = [p.0, p.1, p.4];
                    v.extend((0..arg(2) as usize).map(|k| bytes.get(k).copied().unwrap_or(0)));
                }
                (3, v)
            }
            0x21 => {
                let slots = arg(1) as usize;
                let mut v = Vec::new();
                for s in 0..slots {
                    let c = board.coins.get(s).copied().unwrap_or(0);
                    v.extend_from_slice(&c.to_be_bytes());
                }
                (2, v)
            }
            0x22 => {
                // analog channels: fixed values (the Haunted Museum games read their volume
                // knob on channel 0, 0: silent) or player 1's axes
                (2, (0..arg(1).max(1) as usize).flat_map(|c| analog(board, &sticks[0], c).to_be_bytes()).collect())
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
            0x32 => (arg(1) as usize + 2, vec![]),
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
            0x23 | 0x25 => (2, vec![]),
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
            // commands after it (coins, analogs) are answered
            0x67 => (2, vec![]),
            other => {
                log!("jvs: unknown command {other:#04x}");
                (cmds.len() - i, vec![])
            }
        };
        // commands with outputs/acks: data commands get a report
        // byte when they are not the first one
        if matches!(cmds[i], 0x08 | 0x11..=0x14 | 0x20..=0x22 | 0x26 | 0x2E | 0x30..=0x37 | 0x65 | 0x67) {
            rep(&mut bytes);
        }
        out.extend(&bytes);
        i += len.max(1);
        multi = true;
    }
    out.finish()
}

/// Battle Gear 4's professional cabinet board (node 2): no inputs but the H shifter lanes and
/// mechanism state (`26`), the mechanism motor (`32`); it reports the coin counters too (the
/// game reads the coins from it in this mode).
fn node2(board: &mut Board, cmds: &[u8]) -> Vec<u8> {
    let coins = board.coins;
    let Some(bg) = board.battle_gear.as_mut() else { return Vec::new() };
    let gear = BattleGear::gear(&wal_payload_common::input(1));
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
