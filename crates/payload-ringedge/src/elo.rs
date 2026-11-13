//! An Elo Touchsystems 2701 serial touch controller (Sega 838-14772), `WAL_RINGEDGE_ELO=<port>`
//! (Project DIVA Arcade's touch panel, `COM1`). From segatools' `divahook/elo.c` and
//! `board/elo-frame.c` (public domain; Elo's SmartSet protocol).
//!
//! Every frame is 10 bytes: lead `0x55`, a command letter, 7 data bytes, the sum of the 9 bytes
//! plus `0xAA`. The game queries the acknowledge (`a`: answered `A` and 4 error codes, `0` none),
//! resets (`R`: `0` hard, no answer; `1` soft, answered `R`), queries the serial parameters (`p`:
//! answered `P`, serial, 9600 bps 8-N-1, hardware handshaking, then an `A`) and sets them (`P`,
//! acknowledged). Touched, the controller streams IntelliTouch reports `T`: status (`0x80` with
//! `1` the touch, `2` its stream, `4` its release), X and Y little endian, Z (pressure) `0xFF`.
//!
//! The touch: the touch surface's first finger, else player 1's pointer (a mouse, or the left
//! stick) while b8 is held (the profile maps the mouse button to it); in the controller's units,
//! 0..4095 over the screen, reversed (Project DIVA Arcade reads 4095 minus each coordinate).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use wal_payload_common::{input, log, serial, touches};
use wal_protocol::{Axis, button};

const LEAD: u8 = 0x55;
const FRAME: usize = 10;

static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// Touch reports on (stopped by a hard reset until the next command).
static STARTED: AtomicBool = AtomicBool::new(true);
static TOUCHED: AtomicBool = AtomicBool::new(false);
static LAST_MS: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<(u16, u16)> = Mutex::new((0, 0));

/// Reports while touched: every 10 ms.
const PERIOD_MS: u64 = 10;

/// A frame: lead, `cmd`, `data` (up to 7 bytes, zero padded), checksum.
fn frame(cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; FRAME];
    f[0] = LEAD;
    f[1] = cmd;
    f[2..2 + data.len()].copy_from_slice(data);
    f[FRAME - 1] = f[..FRAME - 1].iter().fold(0xAAu8, |s, b| s.wrapping_add(*b));
    f
}

fn ack() -> Vec<u8> {
    frame(b'A', b"0000")
}

fn reply(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap();
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    loop {
        let Some(start) = rx.iter().position(|&b| b == LEAD) else {
            rx.clear();
            break;
        };
        rx.drain(..start);
        if rx.len() < FRAME {
            break;
        }
        let f: Vec<u8> = rx.drain(..FRAME).collect();
        if f[..FRAME - 1].iter().fold(0xAAu8, |s, b| s.wrapping_add(*b)) != f[FRAME - 1] {
            log!("elo: bad checksum {f:02x?}");
            continue;
        }
        STARTED.store(true, Ordering::Relaxed);
        match f[1] {
            b'a' => out.extend(ack()),
            // hard reset: no answer (the controller reboots), its reports stopped
            b'R' if f[2] == b'0' => STARTED.store(false, Ordering::Relaxed),
            b'R' => out.extend(frame(b'R', &[])),
            // serial, 9600 bps 8-N-1, hardware handshaking
            b'p' => {
                out.extend(frame(b'P', &[b'0', 0b110, 0b100]));
                out.extend(ack());
            }
            b'P' => out.extend(ack()),
            cmd => {
                log!("elo: command {cmd:#04x} {f:02x?}");
                out.extend(frame(b'A', b"5000"));
            }
        }
        log!("elo: {} -> {} bytes", f[1] as char, out.len());
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The touch, if any: (x, y) over 0..65535.
fn touch() -> Option<(u16, u16)> {
    if let Some(&t) = touches().first() {
        return Some(t);
    }
    let p1 = input(0);
    p1.pressed(button::B8).then(|| (p1.axis_u16(Axis::LeftX), p1.axis_u16(Axis::LeftY)))
}

/// The reports since the last poll: one per period while touched, one at the release.
fn stream() -> Vec<u8> {
    if !STARTED.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let now = now_ms();
    let touch = touch();
    let was = TOUCHED.load(Ordering::Relaxed);
    if touch.is_none() && !was || now.saturating_sub(LAST_MS.load(Ordering::Relaxed)) < PERIOD_MS {
        return Vec::new();
    }
    LAST_MS.store(now, Ordering::Relaxed);
    TOUCHED.store(touch.is_some(), Ordering::Relaxed);
    let (status, (x, y)) = match touch {
        Some((x, y)) => {
            let at = (0x0FFF - (x >> 4), 0x0FFF - (y >> 4));
            *LAST.lock().unwrap() = at;
            (if was { 0x82 } else { 0x81 }, at)
        }
        None => (0x84, *LAST.lock().unwrap()),
    };
    let (x, y) = (x & 0x0FFF, y & 0x0FFF);
    frame(b'T', &[status, x as u8, (x >> 8) as u8, y as u8, (y >> 8) as u8, 0xFF, 0])
}

pub(crate) fn init() {
    let Ok(ports) = std::env::var("WAL_RINGEDGE_ELO") else { return };
    for port in ports.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        serial::install_streaming(port, reply, stream);
        log!("elo: 2701 touch controller on {port}");
    }
}
