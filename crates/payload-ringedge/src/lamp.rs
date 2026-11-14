//! Project DIVA Arcade's lamp control board, `WAL_RINGEDGE_DIVA_LAMP=<port>`: a USB serial
//! board the game looks for among the machine's ports not on the main board (the first one the
//! SetupDi enumeration lists with an enumerator other than `ACPI`: `WAL_COMPORTS`), its startup
//! check LAMP CTRL BD NG without it.
//!
//! Every frame starts with `0xE0`, its other bytes escaped: `0xE0` and `0xD0` sent as `0xD0`
//! and the byte minus one. The game sends its length (the command, its data and the checksum),
//! the command, its data and the checksum: the complement of the bytes' sum with the carries
//! added back. The board answers each command with its length, the status `0xF0` (`0xF4` /
//! `0xF5` errors), the command and its data (a string for `0xA1` / `0xA2`), then the checksum;
//! the game sends the next one once answered. Its opening: `0xA0` (reset, not answered), `0xA3`
//! and `0xA4` (`0`), `0xA7` (`08 F2`), `0xA1` and `0xA2` (the board's strings), then the lamps:
//! `0x61` with their 6 bits (the 4 hit buttons, start...) on each update, `0x62` with 6 bytes.
//! A board silent for 300 updates is NG.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use wal_payload_common::{log, serial};

const SYNC: u8 = 0xE0;
const ESCAPE: u8 = 0xD0;

static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// The lamps last sent (`0x61`), logged when they change.
static LAMPS: AtomicU8 = AtomicU8::new(0);

/// The complement of the bytes' sum, its carries added back.
fn checksum(bytes: &[u8]) -> u8 {
    let mut sum = 0u16;
    for &b in bytes {
        sum += b as u16;
        while sum > 0xFF {
            sum = (sum & 0xFF) + (sum >> 8);
        }
    }
    !(sum as u8)
}

/// A frame: the sync byte, then `body` and its checksum, escaped.
fn frame(body: &[u8]) -> Vec<u8> {
    let mut f = vec![SYNC];
    for &b in body.iter().chain(std::iter::once(&checksum(body))) {
        if b == SYNC || b == ESCAPE {
            f.extend([ESCAPE, b - 1]);
        } else {
            f.push(b);
        }
    }
    f
}

/// The answer to `cmd`: its length, the status, the command, `data`.
fn answer(cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut body = vec![data.len() as u8 + 3, 0xF0, cmd];
    body.extend_from_slice(data);
    frame(&body)
}

/// A received frame's body without its checksum, if valid.
fn decode(raw: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::with_capacity(raw.len());
    let mut bytes = raw.iter();
    while let Some(&b) = bytes.next() {
        body.push(if b == ESCAPE { bytes.next()?.wrapping_add(1) } else { b });
    }
    let sum = body.pop()?;
    (body.len() >= 2 && checksum(&body) == sum).then_some(body)
}

fn reply(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap();
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    // complete frames: those followed by the next one's sync byte, and the last one when it
    // decodes (the game sends one frame per write)
    loop {
        let Some(start) = rx.iter().position(|&b| b == SYNC) else {
            rx.clear();
            break;
        };
        rx.drain(..start);
        let end = rx[1..].iter().position(|&b| b == SYNC).map_or(rx.len(), |p| p + 1);
        let Some(body) = decode(&rx[1..end]) else {
            if end < rx.len() {
                log!("lamp: bad frame {:02x?}", &rx[..end]);
                rx.drain(..end);
                continue;
            }
            break;
        };
        rx.drain(..end);
        let (cmd, args) = (body[1], &body[2..]);
        match cmd {
            // reset: the board reboots, no answer
            0xA0 => {}
            0xA1 => out.extend(answer(cmd, b"1.00\0")),
            0xA2 => out.extend(answer(cmd, b"wal\0")),
            0x61 => {
                let lamps = args.first().copied().unwrap_or(0);
                if LAMPS.swap(lamps, Ordering::Relaxed) != lamps {
                    log!("lamp: {lamps:06b}");
                }
                out.extend(answer(cmd, &[]));
            }
            _ => {
                if cmd != 0x62 {
                    log!("lamp: command {cmd:#04x} {args:02x?}");
                }
                out.extend(answer(cmd, &[]));
            }
        }
    }
    out
}

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_RINGEDGE_DIVA_LAMP") else { return };
    let port = port.trim();
    if !port.is_empty() {
        serial::install(port, reply);
        log!("lamp: control board on {port}");
    }
}
