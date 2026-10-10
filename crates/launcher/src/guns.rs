//! Lightguns and mice aiming like lightguns, read with evdev (port of batocera-wine-guns).
//!
//! * Guns: devices udev tags `ID_INPUT_GUN=1` (fallback without udev data: an absolute
//!   pointer, `ABS_X`/`ABS_Y` + `BTN_LEFT`, that is not a touchpad). They take players 1, 2...
//!   in device order (`eventN` number).
//! * Mice (`REL_X`/`REL_Y` + `BTN_LEFT`) fill the remaining players, USB mice first (before a
//!   laptop's touchpad and TrackPoint): their motion moves a position over a virtual screen of
//!   `input.mouse_screen` pixels.
//!
//! Each device reports its position as the `x`/`y` sources (-32768 left/top..32767), its
//! buttons as evdev key sources (`BTN_LEFT`...) and `offscreen` while a gun points off the
//! screen. The devices are not grabbed: the game still gets them as X11 mice.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode, RelativeAxisCode, SynchronizationCode};
use wal_protocol::MAX_PLAYERS;

use crate::mapping::RawKey;

/// Consecutive reports at the edge before a gun counts as off-screen: a Sinden emits isolated
/// 0 / max samples when it briefly loses the border.
const OFFSCREEN_DEBOUNCE: u32 = 3;

/// A device update: (pointer index, input, value).
pub type PointerEvent = (usize, RawKey, i32);

pub struct Pointer {
    pub path: PathBuf,
    pub name: String,
    pub is_gun: bool,
    pub player: usize,
}

/// Value of udev property `key` for an evdev node (`/run/udev/data/c13:<minor>`).
pub fn udev_property(path: &Path, key: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let rdev = std::fs::metadata(path).ok()?.rdev();
    let (major, minor) = ((rdev >> 8) & 0xfff, (rdev & 0xff) | ((rdev >> 12) & 0xfff00));
    let data = std::fs::read_to_string(format!("/run/udev/data/c{major}:{minor}")).ok()?;
    data.lines().find_map(|l| l.strip_prefix("E:")?.strip_prefix(key)?.strip_prefix('=').map(str::to_string))
}

fn is_gun(path: &Path, dev: &Device) -> bool {
    if let Some(v) = udev_property(path, "ID_INPUT_GUN") {
        return v == "1";
    }
    let abs = dev.supported_absolute_axes();
    let keys = dev.supported_keys();
    abs.is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_X) && a.contains(AbsoluteAxisCode::ABS_Y))
        && keys.is_some_and(|k| k.contains(KeyCode::BTN_LEFT) && !k.contains(KeyCode::BTN_TOUCH))
}

/// `N` of `/dev/input/eventN` (enumeration order: `event10` after `event6`).
fn event_number(path: &Path) -> u32 {
    path.file_name().and_then(|n| n.to_str()?.strip_prefix("event")?.parse().ok()).unwrap_or(u32::MAX)
}

fn is_mouse(dev: &Device) -> bool {
    let rel = dev.supported_relative_axes();
    rel.is_some_and(|r| r.contains(RelativeAxisCode::REL_X) && r.contains(RelativeAxisCode::REL_Y))
        && dev.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_LEFT))
}

/// Finds the guns, then the mice for the remaining players, and starts a reader thread for
/// each one, sending its updates to `tx`.
pub fn start(use_mice: bool, max_mice: usize, mouse_screen: [u32; 2], tx: Sender<PointerEvent>) -> Vec<Pointer> {
    let mut devices: Vec<(PathBuf, Device)> = evdev::enumerate().collect();
    devices.sort_by_key(|(p, _)| event_number(p));
    let (guns, others): (Vec<_>, Vec<_>) = devices.into_iter().partition(|(p, d)| is_gun(p, d));
    let mut mice: Vec<_> = others.into_iter().filter(|(_, d)| use_mice && is_mouse(d)).collect();
    // plugged-in (USB) mice before the built-in pointers (touchpad, TrackPoint): with
    // `guns_mice: 1` on a laptop, the external mouse aims
    mice.sort_by_key(|(p, _)| (udev_property(p, "ID_BUS").as_deref() != Some("usb"), event_number(p)));
    let mice = mice.into_iter().take(if max_mice == 0 { usize::MAX } else { max_mice });

    let mut pointers = Vec::new();
    for (path, dev) in guns.into_iter().chain(mice).take(MAX_PLAYERS) {
        let index = pointers.len();
        let is_gun = is_gun(&path, &dev);
        let pointer = Pointer { path, name: dev.name().unwrap_or("?").to_string(), is_gun, player: index };
        let tx = tx.clone();
        if is_gun {
            std::thread::spawn(move || read_gun(index, dev, tx));
        } else {
            std::thread::spawn(move || read_mouse(index, dev, mouse_screen, tx));
        }
        pointers.push(pointer);
    }
    pointers
}

/// `v` in `min..=max` -> -32768..=32767, and whether it is on an edge.
fn normalize(v: i32, min: i32, max: i32) -> (i32, bool) {
    if max <= min {
        return (0, false);
    }
    let n = (v - min) as f64 / (max - min) as f64;
    let edge = n <= 0.0 || n >= 1.0;
    ((n.clamp(0.0, 1.0) * 65535.0).round() as i32 - 32768, edge)
}

fn read_gun(index: usize, mut dev: Device, tx: Sender<PointerEvent>) {
    let range = |code: AbsoluteAxisCode| {
        dev.get_absinfo()
            .ok()
            .and_then(|mut it| it.find(|(c, _)| *c == code))
            .map_or((0, 65535), |(_, i)| (i.minimum(), i.maximum()))
    };
    let ranges = [range(AbsoluteAxisCode::ABS_X), range(AbsoluteAxisCode::ABS_Y)];
    // centered until the first report (the edge would count as off-screen)
    let mut raw = [(ranges[0].0 + ranges[0].1) / 2, (ranges[1].0 + ranges[1].1) / 2];
    let mut edge_streak = 0;
    let mut offscreen = false;
    while let Ok(events) = dev.fetch_events() {
        for ev in events {
            let sent = match ev.destructure() {
                EventSummary::AbsoluteAxis(_, code, value) => {
                    let axis = match code {
                        AbsoluteAxisCode::ABS_X => 0,
                        AbsoluteAxisCode::ABS_Y => 1,
                        _ => continue,
                    };
                    raw[axis] = value;
                    let (v, _) = normalize(value, ranges[axis].0, ranges[axis].1);
                    tx.send((index, RawKey::GunAxis(axis as u8), v))
                }
                EventSummary::Key(_, code, value) if value != 2 => tx.send((index, RawKey::Key(code.code()), value)),
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    let at_edge = (0..2).any(|a| normalize(raw[a], ranges[a].0, ranges[a].1).1);
                    edge_streak = if at_edge { edge_streak + 1 } else { 0 };
                    let now = edge_streak >= OFFSCREEN_DEBOUNCE;
                    if now == offscreen {
                        continue;
                    }
                    offscreen = now;
                    tx.send((index, RawKey::GunOffscreen, now as i32))
                }
                _ => continue,
            };
            if sent.is_err() {
                return;
            }
        }
    }
    eprintln!("input: gun {} disconnected", index + 1);
}

fn read_mouse(index: usize, mut dev: Device, screen: [u32; 2], tx: Sender<PointerEvent>) {
    let size = screen.map(|s| s.max(1) as i32);
    // starts mid-screen
    let mut pos = size.map(|s| s / 2);
    while let Ok(events) = dev.fetch_events() {
        for ev in events {
            let sent = match ev.destructure() {
                EventSummary::RelativeAxis(_, code, value) => {
                    let axis = match code {
                        RelativeAxisCode::REL_X => 0,
                        RelativeAxisCode::REL_Y => 1,
                        _ => continue,
                    };
                    pos[axis] = (pos[axis] + value).clamp(0, size[axis]);
                    let (v, _) = normalize(pos[axis], 0, size[axis]);
                    tx.send((index, RawKey::GunAxis(axis as u8), v))
                }
                EventSummary::Key(_, code, value) if value != 2 => tx.send((index, RawKey::Key(code.code()), value)),
                _ => continue,
            };
            if sent.is_err() {
                return;
            }
        }
    }
    eprintln!("input: mouse {} disconnected", index + 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize(0, 0, 100), (-32768, true));
        assert_eq!(normalize(100, 0, 100), (32767, true));
        assert_eq!(normalize(50, 0, 100), (0, false));
        assert_eq!(normalize(-5, 0, 100), (-32768, true));
        assert_eq!(normalize(5, 5, 5), (0, false));
    }
}
