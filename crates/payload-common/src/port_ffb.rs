//! Force feedback boards on the parallel port (`inpout32.dll`'s `Out32`): Sega Europa-R
//! games (`WAL_EUROPAR_FFB*`) and Showdown (`WAL_SEGASHELL_FFB*`), the payload's [`out`] given
//! its variables' prefix.
//!
//! Ford Racing (the default format):
//!
//! The game (`0x467990`) scales its steering force (-1..=1) by the operator's strength
//! (`[ForceFeedback] Strength` of `calibration.ini`, 0..=31, / 15) to -15..=15 and writes one
//! byte to the data port `0x378`: 0 no force, 1..=15 for a positive force (1 strongest:
//! `16 - force`), 16..=30 for a negative one (16 strongest: `31 + force`). At start and stop
//! it writes `0x10` to the control port `0x37a`, then 0 to the data port.
//!
//! In a race it writes twice per frame, an opposite force then the one that stays until the
//! next frame (`0x4a20c0` then `0x4a2020`): the motor follows the byte that stays. The port is
//! sampled every 4 ms, its byte becomes a constant force (`<prefix>_FFB`, percent, default
//! 100): a positive force pushes the wheel to the right.
//!
//! `<prefix>_FFB_FORMAT=rally3`: SEGA Rally 3 (`0x5c8600`) writes to port 0x1020 the low
//! nibble `16 - |force| * 15` (1 strongest, 0 none) with bit 0x10 set for a positive force;
//! its positive force pushes the wheel to the right too.
//!
//! `<prefix>_FFB_FORMAT=grid`: GRID (`0xb9c900`) writes to port 0x1020 the low nibble
//! `15 - |force| * 15` (at least 1: 15 none, 1 strongest) with bit 0x10 set for a negative
//! force. Showdown (`0xd2dd80`) the same, `15 - |force| * 14` for a positive force.

use std::sync::Once;
use std::sync::atomic::{AtomicI32, AtomicU8, Ordering};
use std::time::Duration;

use std::sync::OnceLock;

use crate::{log, send_output};
use wal_protocol::output;

static LAST: AtomicI32 = AtomicI32::new(i32::MIN);
/// The data port's byte.
static PORT: AtomicU8 = AtomicU8::new(0);
static LOGGED: AtomicI32 = AtomicI32::new(0);
/// The strength, percent (`<prefix>_FFB`).
static PERCENT: OnceLock<i32> = OnceLock::new();

fn percent() -> i32 {
    PERCENT.get().copied().unwrap_or(100)
}

/// SEGA Rally 3's encoding (`<prefix>_FFB_FORMAT=rally3`, port 0x1020).
static RALLY3: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// GRID's encoding (`<prefix>_FFB_FORMAT=grid`, port 0x1020).
static GRID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// GRID's byte -> its force, -15..=15.
fn force_grid(byte: u8) -> i32 {
    let m = 15 - (byte & 0x0f) as i32;
    if byte & 0x10 != 0 { -m } else { m }
}
static DATA_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0x378);

/// SEGA Rally 3's byte -> its force, -15..=15.
fn force_rally3(byte: u8) -> i32 {
    let m = match byte & 0x0f {
        0 => 0,
        n => 16 - n as i32,
    };
    if byte & 0x10 != 0 { m } else { -m }
}

/// The data port byte -> the game's force, -15..=15.
fn force(byte: u8) -> i32 {
    match byte {
        1..=15 => 16 - byte as i32,
        16..=30 => byte as i32 - 31,
        _ => 0,
    }
}

/// `Out32(port, data)` of the game; `prefix` names its variables (`WAL_EUROPAR`: `WAL_EUROPAR_FFB`,
/// `WAL_EUROPAR_FFB_FORMAT`).
pub fn out(prefix: &str, port: u16, data: u16) {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = PERCENT.set(std::env::var(format!("{prefix}_FFB")).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(100));
        let format = std::env::var(format!("{prefix}_FFB_FORMAT")).unwrap_or_default().trim().to_ascii_lowercase();
        match format.as_str() {
            "rally3" => RALLY3.store(true, Ordering::Relaxed),
            "grid" => GRID.store(true, Ordering::Relaxed),
            _ => {}
        }
        if matches!(format.as_str(), "rally3" | "grid") {
            DATA_PORT.store(0x1020, Ordering::Relaxed);
        }
    });
    if port != DATA_PORT.load(Ordering::Relaxed) {
        log!("ffb: Out32({port:#x}, {data:#x})");
        return;
    }
    PORT.store(data as u8, Ordering::Relaxed);
    static SAMPLER: Once = Once::new();
    SAMPLER.call_once(|| {
        std::thread::spawn(|| {
            loop {
                sample();
                std::thread::sleep(Duration::from_millis(4));
            }
        });
    });
}

fn sample() {
    let byte = PORT.load(Ordering::Relaxed);
    let f = if RALLY3.load(Ordering::Relaxed) {
        force_rally3(byte)
    } else if GRID.load(Ordering::Relaxed) {
        force_grid(byte)
    } else {
        force(byte)
    };
    let v = (f * output::FFB_MAX / 15 * percent() / 100).clamp(-output::FFB_MAX, output::FFB_MAX);
    if LAST.swap(v, Ordering::Relaxed) != v {
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 20 {
            log!("ffb: byte {byte:#04x}: constant force {v}");
        }
        send_output(0, output::FFB_CONSTANT, v);
    }
}

#[cfg(test)]
mod tests {
    use super::{force, force_grid, force_rally3};

    #[test]
    fn decodes_the_game_bytes() {
        assert_eq!(force(0), 0);
        assert_eq!(force(1), 15);
        assert_eq!(force(15), 1);
        assert_eq!(force(16), -15);
        assert_eq!(force(30), -1);
    }

    #[test]
    fn decodes_grid_bytes() {
        assert_eq!(force_grid(0x0f), 0);
        assert_eq!(force_grid(0x01), 14);
        assert_eq!(force_grid(0x11), -14);
        assert_eq!(force_grid(0x1f), 0);
    }

    #[test]
    fn decodes_rally3_bytes() {
        assert_eq!(force_rally3(0), 0);
        assert_eq!(force_rally3(0x10), 0);
        assert_eq!(force_rally3(0x11), 15);
        assert_eq!(force_rally3(0x1f), 1);
        assert_eq!(force_rally3(0x01), -15);
    }
}
