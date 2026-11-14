//! A 3M MicroTouch multi-touch serial controller (Wonderland Wars, KanColle Arcade; segatools'
//! carolhook `3mpxsc`, public domain): text commands (`\x01` command `\r`, answered
//! `\x010\r`; the name, identity and unit type asked), then a stream of reports (10 touches,
//! 6 bytes each: status 0x80 and 0x40 while touching, X and Y in 7-bit halves over 0..16383, Y
//! from the bottom, the touch's id). The touch: the launcher's touch surface (a touchscreen, the
//! touchpad), else player 1's gun (the mouse, a lightgun) while `b1` is held.

use std::sync::Mutex;

use crate::{gun, serial};
use wal_protocol::button;

/// The controller on `port`.
pub fn init(port: &str) {
    serial::install_streaming(port, touch_command, touch_report);
}

/// A command (`\x01` name `\r`): acknowledged; the name, identity and unit type asked.
fn touch_command(packet: &[u8]) -> Vec<u8> {
    let cmd = packet.strip_prefix(&[1]).unwrap_or(packet);
    let cmd = cmd.split(|&b| b == b'\r').next().unwrap_or(&[]);
    match cmd {
        b"NM" | b"OI" => b"\x01AD1000\r".to_vec(),
        b"UT" => b"\x01AD****00\r".to_vec(),
        _ => b"\x010\r".to_vec(),
    }
}

/// The last touch's position (window pixels), kept for its release report.
static LAST_TOUCH: Mutex<Option<(u16, u16)>> = Mutex::new(None);

/// One report of the stream (10 touches, the pen in the first), when the game reads the port.
fn touch_report() -> Vec<u8> {
    static POLLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !POLLED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::log!("touch3m: reports polled");
    }
    if crate::input(0).pressed(button::B1) && pen_position().is_none() {
        static MISSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !MISSED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::log!("touch3m: b1 held, no position in the picture");
        }
    }
    let pen = pen_position();
    let mut report = [0u8; 60];
    for touch in report.chunks_mut(6) {
        touch[0] = 0x80;
    }
    let mut last = LAST_TOUCH.lock().unwrap();
    let was_touching = last.is_some();
    let (status, at) = match pen {
        Some(at) => (0xC0, Some(at)),
        None => (0x80, last.take()),
    };
    if let Some((x, y)) = at {
        report[..6].copy_from_slice(&[status, (x & 0x7F) as u8, (x >> 7 & 0x7F) as u8, (y & 0x7F) as u8, (y >> 7 & 0x7F) as u8, 1]);
    }
    if pen.is_some() != was_touching {
        crate::log!("touch3m: {}", match pen { Some((x, y)) => format!("touch at {x}, {y}"), None => "release".into() });
    }
    if pen.is_some() {
        *last = pen;
    }
    report.to_vec()
}

/// The controller's coordinates: 14 bits, Y from the bottom (pixels, 0..1919 and 0..1079,
/// touched only the screen's bottom left corner).
const RANGE: f32 = 16383.0;

/// Where the pen touches, in the controller's coordinates (the touch surface's or the gun's
/// place in the game's picture, whatever the window's size).
fn pen_position() -> Option<(u16, u16)> {
    // each touch lasts at least 350 ms (a click is shorter: the game, reading the controller
    // every so often, missed it)
    static HELD: Mutex<Option<(std::time::Instant, (f32, f32))>> = Mutex::new(None);
    let now = match crate::touches().first() {
        Some(&(x, y)) => Some((x as f32 / 65535.0, y as f32 / 65535.0)),
        None if crate::input(0).pressed(button::B1) => gun::gun_in_picture(),
        None => None,
    };
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let (u, v) = match (now, *held) {
        (Some(at), Some((since, _))) => {
            *held = Some((since, at));
            at
        }
        (Some(at), None) => {
            *held = Some((std::time::Instant::now(), at));
            at
        }
        (None, Some((since, at))) if since.elapsed() < std::time::Duration::from_millis(350) => at,
        (None, _) => {
            *held = None;
            return None;
        }
    };
    Some(((u.clamp(0.0, 1.0) * RANGE) as u16, ((1.0 - v.clamp(0.0, 1.0)) * RANGE) as u16))
}
