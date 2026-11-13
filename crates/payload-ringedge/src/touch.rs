//! The touch panel (`WAL_RINGEDGE_TOUCH=<port>`, e.g. `COM1`): Sega's CTR-251500 serial touch
//! controller, driven by the game's amptw library (Puyo Puyo!! Quest Arcade).
//!
//! Packets both ways are 10 bytes: `0x55`, a command letter, 7 data bytes, a checksum (the sum
//! of the 9 bytes before it, plus 0xAA). Requests are lower case
//! (`a` ack, `d` diagnostics...) or `P` (port setup: `30 <baud> 04`); the controller answers
//! with the letter in upper case and the same data, which the library accepts (`P` and `p`, its
//! query: `30 <baud> 04`).
//!
//! While touched the controller streams `T` reports (Puyo Puyo!! Quest reads COM1 itself, its
//! parser at 0x982830, the `T` handler at 0x982000): a flags byte (1 the touch, 2 still
//! touched, 4 the release), X and Y little-endian, three unused bytes. The touch: player 1's
//! pointer (a mouse acting as a gun, or the left stick) with b8 (the profile maps the mouse
//! button to it). The game takes the panel's 0-4096 to its screen (`WAL_RINGEDGE_TOUCH_SCREEN`,
//! default 1360x768), Y from the bottom, then applies its two-point calibration: hod5.ini's
//! [Touch Panel] p1 and p2, the screen positions read at the screen's top left and bottom right
//! (0x4538a1; the place is (read - p1) / ((p2 - p1) / screen size)). The dump's p1 1280,86 and
//! p2 83,674 mirror X and shrink both axes: the reports carry the pointer's place through that
//! calibration's inverse (`WAL_RINGEDGE_TOUCH_CALIBRATION=<p1 x>,<p1 y>,<p2 x>,<p2 y>`, default
//! none: the screen as is).

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::OnceLock;

use wal_payload_common::{input, log, serial};
use wal_protocol::{Axis, button};

/// The baud rate code of the last `P` (`p` queries it).
static BAUD: AtomicU8 = AtomicU8::new(6);
/// Touched at the last report, and the time of that report (ms).
static TOUCHED: AtomicBool = AtomicBool::new(false);
static LAST_MS: AtomicU64 = AtomicU64::new(0);
/// The screen's size and the calibration's p1, p2 (game pixels).
static CAL: OnceLock<Calibration> = OnceLock::new();

struct Calibration {
    size: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
}

impl Calibration {
    fn from_env() -> Calibration {
        let size = std::env::var("WAL_RINGEDGE_TOUCH_SCREEN")
            .ok()
            .and_then(|v| v.trim().split_once('x').and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?))))
            .unwrap_or((1360.0, 768.0));
        let points: Vec<f32> = std::env::var("WAL_RINGEDGE_TOUCH_CALIBRATION")
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        let (p1, p2) = match points[..] {
            [x1, y1, x2, y2] => ((x1, y1), (x2, y2)),
            _ => ((0.0, 0.0), size),
        };
        Calibration { size, p1, p2 }
    }

    /// The panel's report (0-4096, Y from the bottom) the game takes to `(u, v)`, the screen's
    /// fractions from its top left.
    fn report(&self, u: f32, v: f32) -> (u16, u16) {
        let sx = self.p1.0 + (self.p2.0 - self.p1.0) * u;
        let sy = self.p1.1 + (self.p2.1 - self.p1.1) * v;
        let x = sx / self.size.0 * 4096.0;
        let y = 4096.0 - sy / self.size.1 * 4096.0;
        (x.round().clamp(0.0, 4095.0) as u16, y.round().clamp(0.0, 4095.0) as u16)
    }
}

/// Reports while touched: every 10 ms.
const PERIOD_MS: u64 = 10;

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_RINGEDGE_TOUCH") else { return };
    let cal = CAL.get_or_init(Calibration::from_env);
    serial::install_streaming(port.trim(), reply, stream);
    log!("touch: CTR-251500 panel on {}, screen {:?}, calibration {:?} {:?}", port.trim(), cal.size, cal.p1, cal.p2);
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

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The touch reports since the last poll: one at the touch, one per period while touched, one
/// at the release.
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
    let u = p1.axis_u16(Axis::LeftX) as f32 / 65535.0;
    let v = p1.axis_u16(Axis::LeftY) as f32 / 65535.0;
    let (x, y) = CAL.get_or_init(Calibration::from_env).report(u, v);
    let state = match (was, down) {
        (false, true) => 0x01,
        (true, true) => 0x02,
        _ => 0x04,
    };
    let mut r = vec![0x55, b'T', state, x as u8, (x >> 8) as u8, y as u8, (y >> 8) as u8, 0, 0, 0];
    r[9] = checksum(&r);
    if down != was {
        log!("touch: {} at {x},{y}", if down { "touch" } else { "release" });
    }
    r
}
