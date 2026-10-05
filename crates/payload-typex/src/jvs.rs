//! JVS I/O board for Taito Type X: Taito stick mode (2 players, 16 switches), JVS version 0x30.
//!
//! Replies: one report byte for the packet, then each command's data; the commands after
//! the first add their own report byte.
//!
//! Switch byte 1: start 0x80, service 0x40, up 0x20, down 0x10, left 0x08, right 0x04,
//! button 1 0x02, button 2 0x01. Switch byte 2: buttons 3-6 0x80/0x40/0x20/0x10.
//! System byte: test 0x80. Coins count up when the coin input is released.

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use wal_payload_common::jvs::{Encoder, parse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{log, serial};
use wal_protocol::StickState;

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
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

pub(crate) fn init() {
    *BOARD.lock().unwrap() = Some(Board { map: ButtonMap::new(NATIVES, DEFAULT_MAP), coins: [0; 2], coin_held: [false; 2] });
    let port = std::env::var("WAL_TYPEX_JVS_PORT").unwrap_or_else(|_| "COM2".into());
    serial::install(&port, process);
    log!("jvs: I/O board on {port}");
}

/// (switch byte 1, switch byte 2, test, coin) of a player.
fn player(map: &ButtonMap<Native>, stick: &StickState) -> (u8, u8, bool, bool) {
    let (mut b1, mut b2, mut test, mut coin) = (0u8, 0u8, false, false);
    map.for_each_pressed(stick, |n| match n {
        Native::Start => b1 |= 0x80,
        Native::Service => b1 |= 0x40,
        Native::Up => b1 |= 0x20,
        Native::Down => b1 |= 0x10,
        Native::Left => b1 |= 0x08,
        Native::Right => b1 |= 0x04,
        Native::Btn(1) => b1 |= 0x02,
        Native::Btn(2) => b1 |= 0x01,
        Native::Btn(n) => b2 |= 0x80 >> (n - 3),
        Native::Test => test = true,
        Native::Coin => coin = true,
    });
    (b1, b2, test, coin)
}

fn process(packet: &[u8]) -> Vec<u8> {
    let Some(req) = parse(packet) else { return Vec::new() };
    // only the broadcast and our own node (1)
    if req.node != 0xFF && req.node > 0x01 {
        return Vec::new();
    }
    let cmds = req.commands;
    // bus reset: no reply
    if cmds.first() == Some(&0xF0) {
        return Vec::new();
    }
    let mut guard = BOARD.lock().unwrap();
    let Some(board) = guard.as_mut() else { return Vec::new() };

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
                serial::READY.store(true, Ordering::Relaxed);
                (2, vec![])
            }
            0x10 => (1, IDENTIFIER.to_vec()),
            0x11 => (1, vec![COMMAND_REVISION]),
            0x12 => (1, vec![JVS_VERSION]),
            0x13 => (1, vec![COMM_VERSION]),
            // features, Taito stick: 2 players x 16 switches, 2 coin slots
            0x14 => (1, vec![0x01, 0x02, 0x10, 0x00, 0x02, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
            0x15 => (cmds[i..].iter().position(|b| *b == 0).map_or(cmds.len() - i, |p| p + 1), vec![0x01, 0x01, 0x05]),
            0x20 if (arg(1), arg(2)) == (2, 2) || arg(1) == 0 => {
                // Taito stick: system byte + 2 bytes per player for both players
                let p1 = players[0];
                let p2 = players[1];
                (3, vec![if test { 0x80 } else { 0 }, p1.0, p1.1, p2.0, p2.1])
            }
            0x20 => {
                // other layouts (Gaia Attack 4: 1 player x 3 bytes), generic
                // reply: the 2 switch bytes of each player, padded with zeros
                let mut v = vec![if test { 0x80 } else { 0 }];
                for p in players.iter().take(arg(1).min(2) as usize) {
                    let bytes = [p.0, p.1];
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
            0x22 => (2, vec![0; arg(1).max(1) as usize * 2]),
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
            0x23 | 0x25 => (2, vec![]),
            0x65 => (2, vec![0xA0]),
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
        if matches!(cmds[i], 0x11..=0x14 | 0x20..=0x22 | 0x26 | 0x2E | 0x30..=0x37 | 0x65 | 0x67) {
            rep(&mut bytes);
        }
        out.extend(&bytes);
        i += len.max(1);
        multi = true;
    }
    out.finish()
}
