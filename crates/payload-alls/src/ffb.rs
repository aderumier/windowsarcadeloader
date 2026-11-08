//! Sega World Drivers Championship's force feedback board on COM1: 4-byte commands
//! `cmd p1 p2 sum` (sum: the XOR of the three bytes, 7 bits), never answered (segatools'
//! `board/ffb.c` for the commands' meaning). `FF 00 00` at start, `FA 00 10` a second later,
//! then one command per frame (`FD 00 00` while nothing changes):
//!
//! - `80 00 xx`: the motor on (`xx` 1) or off (every force stopped)
//! - `84 dd ff`: a constant force, `ff` 0-127: direction 0 (to the left) with `128 - ff`,
//!   else to the right with `ff`
//! - `85 ff pp`: a vibration (road surface, collisions), strength `ff`, period `pp`
//! - `86 00 ff`: the damper: the wheel's resistance, `ff` 0-127 (a centering spring here)
//!
//! Sent to the launcher as its force feedback outputs (the player's wheel or pad).

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use wal_payload_common::{log, serial};
use wal_protocol::output::{FFB_CONSTANT, FFB_MAX, FFB_SPRING, FFB_VIBRATION};

static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// The commands seen (the first of each in the log).
static SEEN: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// The last value sent of each output (constant, vibration, spring): sent on change only.
static LAST: [AtomicI32; 3] = [const { AtomicI32::new(0) }; 3];

fn send_output(_player: u8, id: u16, value: i32) {
    let slot = match id {
        FFB_CONSTANT => 0,
        FFB_VIBRATION => 1,
        _ => 2,
    };
    if LAST[slot].swap(value, Ordering::Relaxed) != value {
        wal_payload_common::send_output(0, id, value);
    }
}

/// 0-127 scaled to the outputs' full scale.
fn scale(v: u8) -> i32 {
    v.min(127) as i32 * FFB_MAX / 127
}

fn command(c: [u8; 4]) {
    {
        let mut seen = SEEN.lock().unwrap();
        if !seen.contains(&c[0]) {
            seen.push(c[0]);
            log!("ffb: command {:02X?}", c);
        }
    }
    match c[0] {
        0x80 if c[2] != 1 => {
            send_output(0, FFB_CONSTANT, 0);
            send_output(0, FFB_VIBRATION, 0);
            send_output(0, FFB_SPRING, 0);
        }
        0x84 => {
            let force = if c[1] == 0 { -scale(128u8.saturating_sub(c[2])) } else { scale(c[2]) };
            send_output(0, FFB_CONSTANT, force);
        }
        0x85 => send_output(0, FFB_VIBRATION, scale(c[1])),
        0x86 => send_output(0, FFB_SPRING, scale(c[2])),
        _ => {}
    }
}

fn packet(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap();
    rx.extend_from_slice(data);
    while rx.len() >= 4 {
        let c = [rx[0], rx[1], rx[2], rx[3]];
        if (c[0] ^ c[1] ^ c[2]) & 0x7F != c[3] {
            rx.remove(0);
            continue;
        }
        rx.drain(..4);
        command(c);
    }
    Vec::new()
}

pub(crate) fn init() {
    serial::install("COM1", packet);
}
