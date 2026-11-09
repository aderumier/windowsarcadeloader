//! Multi-touch surfaces (profile `input.touch`): a touchscreen, else a touchpad, read finger by
//! finger with evdev (the kernel's multi-touch protocol B: `ABS_MT_SLOT`, `ABS_MT_TRACKING_ID`,
//! `ABS_MT_POSITION_X` / `_Y`). Each report gives the fingers down, normalized from 0 (left /
//! top) to 65535 over the device's range: the game's touch points (`Message::Touch`, CHUNITHM's
//! slider). The device is grabbed: the desktop's cursor does not move, no click reaches it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use evdev::{AbsoluteAxisCode, Device, EventSummary, PropType, SynchronizationCode};
use wal_protocol::MAX_TOUCHES;

/// The fingers down on the touch surface, shared with the reader thread.
#[derive(Clone, Default)]
pub struct Touches(Arc<Mutex<Vec<(u16, u16)>>>);

impl Touches {
    pub fn get(&self) -> Vec<(u16, u16)> {
        self.0.lock().unwrap().clone()
    }
}

/// A multi-touch device (protocol B), touchscreen first: (is a touchscreen).
fn multitouch(dev: &Device) -> Option<bool> {
    let abs = dev.supported_absolute_axes()?;
    let mt = [AbsoluteAxisCode::ABS_MT_SLOT, AbsoluteAxisCode::ABS_MT_TRACKING_ID, AbsoluteAxisCode::ABS_MT_POSITION_X, AbsoluteAxisCode::ABS_MT_POSITION_Y];
    mt.iter().all(|c| abs.contains(*c)).then(|| dev.properties().contains(PropType::DIRECT))
}

/// Opens the touch surface (a touchscreen, else the first touchpad) and reads it in a thread.
pub fn start() -> Touches {
    let touches = Touches::default();
    let mut found: Vec<(bool, PathBuf, Device)> = evdev::enumerate().filter_map(|(p, d)| multitouch(&d).map(|screen| (screen, p, d))).collect();
    // touchscreens first, then device order
    found.sort_by_key(|(screen, p, _)| (!screen, p.clone()));
    let Some((screen, path, dev)) = found.into_iter().next() else {
        eprintln!("input: no multi-touch touchpad or touchscreen found");
        return touches;
    };
    eprintln!("input: {} '{}' ({}) -> touch points", if screen { "touchscreen" } else { "touchpad" }, dev.name().unwrap_or("?"), path.display());
    let shared = touches.clone();
    std::thread::spawn(move || read(dev, &path, shared));
    touches
}

fn read(mut dev: Device, path: &Path, touches: Touches) {
    if let Err(e) = dev.grab() {
        eprintln!("input: {}: not grabbed ({e}): the desktop also sees the touches", path.display());
    }
    let info = |code: AbsoluteAxisCode| {
        dev.get_absinfo().ok().and_then(|mut it| it.find(|(c, _)| *c == code)).map(|(_, i)| (i.minimum(), i.maximum()))
    };
    let (rx, ry) = (info(AbsoluteAxisCode::ABS_MT_POSITION_X).unwrap_or((0, 4095)), info(AbsoluteAxisCode::ABS_MT_POSITION_Y).unwrap_or((0, 4095)));
    let slots = info(AbsoluteAxisCode::ABS_MT_SLOT).map_or(MAX_TOUCHES, |(_, max)| (max.max(0) as usize + 1).min(32));
    let norm = |v: i32, (min, max): (i32, i32)| (((v - min).max(0) as i64 * 65535) / ((max - min).max(1) as i64)).min(65535) as u16;
    // each slot: its finger's position while down
    let mut fingers: Vec<Option<(i32, i32)>> = vec![None; slots];
    let mut slot = 0usize;
    while let Ok(events) = dev.fetch_events() {
        for ev in events {
            match ev.destructure() {
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_SLOT, v) => slot = (v.max(0) as usize).min(slots - 1),
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, v) => {
                    fingers[slot] = if v < 0 { None } else { Some(fingers[slot].unwrap_or((rx.0, ry.0))) };
                }
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_X, v) => {
                    if let Some(f) = &mut fingers[slot] {
                        f.0 = v;
                    }
                }
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_Y, v) => {
                    if let Some(f) = &mut fingers[slot] {
                        f.1 = v;
                    }
                }
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    let points: Vec<(u16, u16)> = fingers.iter().flatten().take(MAX_TOUCHES).map(|&(x, y)| (norm(x, rx), norm(y, ry))).collect();
                    *touches.0.lock().unwrap() = points;
                }
                _ => {}
            }
        }
    }
    eprintln!("input: {}: touch surface gone", path.display());
}
