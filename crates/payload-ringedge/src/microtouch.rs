//! A 3M MicroTouch EX II (7720SC) serial touch controller (`WAL_RINGEDGE_MICROTOUCH=<port>`,
//! `COM1`), driven by the game's amptw library (`ampt_exii7720sc`, Lady Luck).
//!
//! Commands are `SOH <letters> CR`; the controller answers `SOH 0 CR` (ok), except `NM` (its
//! name, holding `Rev <version>`), `OI` and `UV` (identity strings) and `DX` (diagnostics,
//! `SOH 0 CR`: no error). The library sets the format tablet mode (`FT`) and stream mode
//! (`MS`): while touched the controller sends 5-byte reports, `C0` (`80` at the release), X
//! low 7 bits, high 7 bits, then Y, 14-bit coordinates.
//!
//! The touch: player 1's pointer (a mouse acting as a gun, or the left stick) with b8 (the
//! profile maps the mouse button to it). Y grows upwards on the controller.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use wal_payload_common::{input, log, serial};
use wal_protocol::{Axis, button};

/// Touched at the last report, and the time of that report (ms).
static TOUCHED: AtomicBool = AtomicBool::new(false);
static LAST_MS: AtomicU64 = AtomicU64::new(0);

/// Reports while touched: every 10 ms (the controller streams about 100 per second).
const PERIOD_MS: u64 = 10;

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_RINGEDGE_MICROTOUCH") else { return };
    serial::install_streaming(port.trim(), reply, stream);
    log!("microtouch: EX II touch controller on {}", port.trim());
}

fn reply(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for cmd in data.split(|&b| b == 0x01).filter_map(|c| c.strip_suffix(b"\r")) {
        let answer: &[u8] = match cmd {
            b"NM" => b"MicroTouch EX II 7720SC Rev 01.21",
            b"OI" => b"Q10121",
            b"UV" => b"01210012",
            _ => b"0",
        };
        // not the null command, the library's keep-alive
        if cmd != b"Z" {
            log!("microtouch: {} -> {}", String::from_utf8_lossy(cmd), String::from_utf8_lossy(answer));
        }
        out.push(0x01);
        out.extend_from_slice(answer);
        out.push(b'\r');
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The touch reports since the last poll: one per period while touched, one at the release.
fn stream() -> Vec<u8> {
    let p1 = input(0);
    let down = p1.pressed(button::B8);
    let was = TOUCHED.load(Ordering::Relaxed);
    if !down && !was {
        return Vec::new();
    }
    let now = now_ms();
    if down && was && now.saturating_sub(LAST_MS.load(Ordering::Relaxed)) < PERIOD_MS {
        return Vec::new();
    }
    TOUCHED.store(down, Ordering::Relaxed);
    LAST_MS.store(now, Ordering::Relaxed);
    let x = (p1.axis_u16(Axis::LeftX) >> 2) as u16;
    let y = 0x3FFF - (p1.axis_u16(Axis::LeftY) >> 2) as u16;
    vec![if down { 0xC0 } else { 0x80 }, (x & 0x7F) as u8, (x >> 7) as u8, (y & 0x7F) as u8, (y >> 7) as u8]
}
