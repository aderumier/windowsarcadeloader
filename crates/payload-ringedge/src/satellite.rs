//! Sega medal satellite boards (Lady Luck): the cabinet's I/O board (`WAL_RINGEDGE_MEDAL_IO=
//! <port>`, `COM3`) and its control panel board (`WAL_RINGEDGE_CP_BOARD=<port>`, `COM5`).
//!
//! The I/O board is Medal no Gunman's (`gunman.rs`: medals, hopper, lamps) with this cabinet's
//! switches. Its 3 switch bytes: byte 0 bit 3 the selector panel (1: closed), byte 1 bit 0 the
//! reset key, bit 1 the transfer button, bit 2 the main door (1: closed).
//!
//! The C/P board (115200 baud) frames as the I/O board (`E0`, `D0` escapes) with a source:
//! `E0`, destination, source, length, data, checksum (the sum from the destination on). Requests
//! (to 2, from 1) hold commands, the reply (to 1, from 2) a status (1) then per command its byte
//! again, a report (1: ok) and its data, as the game's parser (App_Release.exe 0x430c50) reads:
//!
//! * `10` (2 bytes), `80` (4), `82` (2), `83` (the lamps, the rest of the packet): nothing.
//! * `81` (3 bytes): a byte, then the switches, active low: bit 6 payout, bits 5-1 select 1-5,
//!   bit 0 start, bit 7 the belly box door (1: open); then 8 bytes.
//! * `90`: 9 big endian u32 counters.
//! * `F0`: the firmware version, `<major>-<minor>` in ASCII ended by `FF`, a byte and a u16.
//! * `F1`: 17 bytes. `FE`: nothing.

use wal_payload_common::{input, log, serial};
use wal_protocol::button;

use crate::gunman;

pub(crate) fn init() {
    if let Ok(port) = std::env::var("WAL_RINGEDGE_MEDAL_IO") {
        gunman::install(port.trim(), io_switches);
        log!("satellite: medal I/O board on {}", port.trim());
    }
    if let Ok(port) = std::env::var("WAL_RINGEDGE_CP_BOARD") {
        serial::install(port.trim(), cp_reply);
        log!("satellite: control panel board on {}", port.trim());
    }
}

/// Any player's button.
fn pressed(b: u32) -> bool {
    (0..2).any(|p| input(p).pressed(b))
}

/// The I/O board's switches: doors closed; the reset key (clears errors) on the service button,
/// the transfer button on b7.
fn io_switches() -> [u8; 3] {
    let mut s = [0x08u8, 0x04, 0x00];
    if pressed(button::SERVICE) {
        s[1] |= 0x01;
    }
    if pressed(button::B7) {
        s[1] |= 0x02;
    }
    s
}

/// The control panel's switches (`81`), active low, the belly box door closed: payout b6,
/// select 1-5 b1-b5, start.
fn cp_switches() -> u8 {
    let buttons = [(6, button::B6), (5, button::B1), (4, button::B2), (3, button::B3), (2, button::B4), (1, button::B5), (0, button::START)];
    buttons.iter().fold(0x7Fu8, |s, &(bit, b)| if pressed(b) { s & !(1 << bit) } else { s })
}

/// Request length of a command, with its byte (`83`, the lamps: the rest of the packet).
fn request_len(cmd: u8, rest: usize) -> usize {
    match cmd {
        0x80 => 4,
        0x81 => 3,
        0x10 | 0x82 => 2,
        0x83 => rest,
        _ => 1,
    }
}

/// A command's reply: its byte, the report, its data.
fn answer(cmd: u8) -> Vec<u8> {
    let mut r = vec![cmd, 1];
    match cmd {
        0x81 => {
            r.extend_from_slice(&[0, cp_switches()]);
            r.extend_from_slice(&[0; 8]);
        }
        0x90 => r.extend_from_slice(&[0; 36]),
        0xF0 => r.extend_from_slice(b"1-0\xFF\x00\x00\x00"),
        0xF1 => r.extend_from_slice(&[0; 17]),
        0xFE => r.truncate(1),
        _ => {}
    }
    r
}

fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut it = data.iter();
    while let Some(&b) = it.next() {
        out.push(if b == 0xD0 { it.next().map_or(b, |n| n.wrapping_add(1)) } else { b });
    }
    out
}

/// A reply packet from board 2 to `to`: status 1 then `body`.
fn frame(to: u8, body: &[u8]) -> Vec<u8> {
    let mut packet = vec![to, 0x02, body.len() as u8 + 1, 0x01];
    packet.extend_from_slice(body);
    packet.push(packet.iter().fold(0u8, |s, b| s.wrapping_add(*b)));
    let mut out = vec![0xE0];
    for b in packet {
        if b == 0xE0 || b == 0xD0 {
            out.extend_from_slice(&[0xD0, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

fn cp_reply(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for raw in data.split(|&b| b == 0xE0).filter(|p| !p.is_empty()) {
        let p = unescape(raw);
        // destination, source, length, commands, checksum
        if p.len() < 4 || p[2] as usize + 4 != p.len() {
            log!("satellite: bad C/P packet {raw:02X?}");
            continue;
        }
        let commands = &p[3..p.len() - 1];
        let mut body = Vec::new();
        let mut i = 0;
        while i < commands.len() {
            body.extend_from_slice(&answer(commands[i]));
            i += request_len(commands[i], commands.len() - i);
        }
        out.extend_from_slice(&frame(p[1], &body));
    }
    out
}
