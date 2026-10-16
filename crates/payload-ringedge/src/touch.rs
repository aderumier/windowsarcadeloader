//! The touch panel (`WAL_RINGEDGE_TOUCH=<port>`, e.g. `COM1`): Sega's CTR-251500 serial touch
//! controller, driven by the game's amptw library (Puyo Puyo!! Quest Arcade).
//!
//! Packets both ways are 10 bytes: `0x55`, a command letter, 7 data bytes, a checksum (the sum
//! of the 9 bytes before it, plus 0xAA). Requests are lower case
//! (`a` ack, `d` diagnostics...) or `P` (port setup: `30 <baud> 04`); the controller answers
//! with the letter in upper case and the same data, which the library accepts (`P` and `p`, its
//! query: `30 <baud> 04`). Touch reports (`T`: state flags, 16-bit X, Y, pressure) are not sent: the
//! game can read the mouse instead.

use std::sync::atomic::{AtomicU8, Ordering};

use wal_payload_common::{log, serial};

/// The baud rate code of the last `P` (`p` queries it).
static BAUD: AtomicU8 = AtomicU8::new(6);

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_RINGEDGE_TOUCH") else { return };
    serial::install(port.trim(), reply);
    log!("touch: CTR-251500 panel on {}", port.trim());
}

fn checksum(p: &[u8]) -> u8 {
    p[..9].iter().fold(0xAAu8, |s, b| s.wrapping_add(*b))
}

fn reply(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in data.chunks_exact(10).filter(|p| p[0] == 0x55) {
        let mut r = p.to_vec();
        match p[1] {
            b'P' => BAUD.store(p[3], Ordering::Relaxed),
            // port setup query: 30 <baud> 04
            b'p' => r[2..5].copy_from_slice(&[0x30, BAUD.load(Ordering::Relaxed), 0x04]),
            _ => {}
        }
        r[1] = r[1].to_ascii_uppercase();
        r[9] = checksum(&r);
        log!("touch: {p:02X?} -> {r:02X?}");
        out.extend_from_slice(&r);
    }
    out
}
