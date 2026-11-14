//! `WAL_RINGEDGE_IC_CARD=<port>` (e.g. `COM2`): Border Break Scramble's IC card reader/writer
//! on that serial port, without a card. The game checks it at boot ("CHECKING IC CARD R/W"):
//! no answer to its wake-up within about 3 seconds is "error 23" (the reader's error bit in the
//! card slot manager, FUN_004f2ad0).
//!
//! Protocol (nrs.exe FUN_00670f70, FUN_008848d0): the host writes a raw command of 1 to 3
//! bytes, the reader answers a status byte (`0A` done) and, for some statuses, more bytes; no
//! framing, no checksum. Boot: `F7` wake-up, `68 06 40`, `68 01 xx`, `48` (status: `1A 40`, bit
//! 0x40 the reader ready), `B8 CF` (then the host goes to 38400 baud), each answered `0A`.
//! The game sends nothing more while idle; a card read starts with `68 01 xx` then `28` (card
//! detection), answered here `5A`: no card.

use wal_payload_common::{log, serial};

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_RINGEDGE_IC_CARD") else { return };
    let port = port.trim();
    if port.is_empty() || port == "0" {
        return;
    }
    serial::install(port, reply);
    log!("iccard: IC card reader/writer on {port}, no card");
}

/// The bytes of the command being written (the game writes them one at a time).
static COMMAND: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

/// A command's length from its first byte: `68 <setting> <value>`, `B8 <baud>`, else 1.
fn length(command: u8) -> usize {
    match command {
        0x68 => 3,
        0xB8 => 2,
        _ => 1,
    }
}

fn reply(bytes: &[u8]) -> Vec<u8> {
    let mut command = COMMAND.lock().unwrap();
    let mut out = Vec::new();
    for &b in bytes {
        command.push(b);
        if command.len() < length(command[0]) {
            continue;
        }
        out.extend(match command[0] {
            // reader status: ready (bit 0x40)
            0x48 => vec![0x1A, 0x40],
            // card detection: none
            0x28 => vec![0x5A],
            // wake-up, settings, baud rate: done
            _ => vec![0x0A],
        });
        command.clear();
    }
    out
}
