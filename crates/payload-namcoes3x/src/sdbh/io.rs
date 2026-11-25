//! The cabinet's I/O board: a Namco NeoJV on JVS, behind neodcd's own I/O library
//! (`app\wajvio.dll`, its serial port answered by payload-common's JVS board).
//!
//! neodcd sorts boards by their identification: a plain NA-JV is an old board it refuses
//! (error 3-2), the NeoJV is the current one. It reads the game's switches from JVS lines that
//! do not carry their names on this cabinet:
//! - red (decide) = player 1 START, green (left) = the system TEST bit, blue (right) = player 1
//!   DOWN;
//! - player 2's red, green, blue = player 1 RIGHT, LEFT, button 1;
//! - SERVICE = player 1 SERVICE, TEST = player 1 UP;
//! - the test menu's select up / down / enter = player 1 buttons 8, 9, 10.
//!
//! Analog channel 0 (neodcd's volume) rests at 0x8000.
//!
//! Virtual sticks: b1 red, b2 green, b3 blue (left / right too: green / blue, choosing in
//! the menus), up / down the test menu's select, b4 its enter; player 2 the same on its own
//! stick; coin, service, test.

use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::{input, jvs_board, log};
use wal_protocol::button;

/// The game's buttons held back (the card binder open over the game).
pub static BLOCKED: AtomicBool = AtomicBool::new(false);

const DEFAULTS: [(&str, &str); 4] = [
    ("WAL_TYPEX_JVS_IDENTIFIER", "namco ltd.;NeoJV;Ver1.00;JPN"),
    ("WAL_TYPEX_JVS_DLLS", "wajvio.dll"),
    ("WAL_TYPEX_JVS_ANALOG", "8000"),
    ("WAL_TYPEX_JVS_SWITCHES", "1x24"),
];

pub fn init() {
    for (var, value) in DEFAULTS {
        if std::env::var_os(var).is_none() {
            unsafe { std::env::set_var(var, value) };
        }
    }
    jvs_board::set_switches_hook(switches);
    jvs_board::set_command_hook(namco_command);
    jvs_board::init("COM1,COM2,COM3,COM4");
    log!("sdbh: I/O board NeoJV");
}

/// Namco's `70` commands, the board's serial side channel (neodcd's `NeoDcdJVSio*`): `70 18
/// ...` writes (acknowledged, up to the next command `70`), `70 15 <offset> <form>` reads
/// (nothing received).
fn namco_command(cmds: &[u8]) -> Option<(usize, Vec<u8>)> {
    if cmds.first() != Some(&0x70) {
        return None;
    }
    match cmds.get(1) {
        Some(0x18) => Some((cmds[1..].iter().position(|&b| b == 0x70).map_or(cmds.len(), |n| n + 1), vec![0x01, 0x00])),
        Some(0x15) => Some((4.min(cmds.len()), vec![0x01, 0x00])),
        _ => None,
    }
}

/// The cabinet's lines from the players' sticks (the coins as counted by the board).
fn switches(players: &mut [jvs_board::Switches]) {
    let (p1, p2) = (input(0), input(1));
    let blocked = BLOCKED.load(Ordering::Relaxed);
    let game = |s: &wal_protocol::StickState, b: u32| !blocked && s.pressed(b);
    let red = game(&p1, button::B1);
    let green = game(&p1, button::B2) || game(&p1, button::LEFT);
    let blue = game(&p1, button::B3) || game(&p1, button::RIGHT);
    let p2_red = game(&p2, button::B1);
    let p2_green = game(&p2, button::B2) || game(&p2, button::LEFT);
    let p2_blue = game(&p2, button::B3) || game(&p2, button::RIGHT);
    let bit = |on: bool, v: u8| if on { v } else { 0 };
    let byte1 = bit(red, 0x80)
        | bit(p1.pressed(button::SERVICE), 0x40)
        | bit(p1.pressed(button::TEST), 0x20)
        | bit(blue, 0x10)
        | bit(p2_green, 0x08)
        | bit(p2_red, 0x04)
        | bit(p2_blue, 0x02);
    let byte2 = bit(game(&p1, button::UP), 0x04) | bit(game(&p1, button::DOWN), 0x02) | bit(game(&p1, button::B4), 0x01);
    let coin = players.first().is_some_and(|p| p.3);
    if let Some(p) = players.get_mut(0) {
        *p = (byte1, byte2, green, coin, 0);
    }
    for p in players.iter_mut().skip(1) {
        *p = (0, 0, false, p.3, 0);
    }
}
