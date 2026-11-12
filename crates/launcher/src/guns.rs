//! Lightguns and mice aiming like lightguns, read with evdev (port of batocera-wine-guns).
//!
//! * Guns: devices udev tags `ID_INPUT_GUN=1`. When none is tagged (no Batocera gun rules), an
//!   absolute pointer, `ABS_X`/`ABS_Y` + `BTN_LEFT`, that is not a touchpad nor a virtual
//!   device (Wolf's/Sunshine's streaming pointers). With tags, only them: a Sinden's raw mouse
//!   node (untagged, beside its tagged virtual gun) would be a second gun of the same player.
//!   They take players 1, 2... in device order (`eventN` number).
//! * Mice (`REL_X`/`REL_Y` + `BTN_LEFT`) fill the remaining players, USB mice first (before a
//!   laptop's touchpad and TrackPoint): their motion moves a position over a virtual screen of
//!   `input.mouse_screen` pixels. A touchpad's own mouse node (same HID device as the
//!   touchpad: ELAN i2c touchpads have one, it never moves) is skipped, so a laptop's single
//!   mouse is its TrackPoint.
//!
//! * With `input.mouse_wrap` (games reading a trackball): every mouse and touchpad (a finger's
//!   motion) rolls one position of player 1 that wraps around the virtual screen, the
//!   trackball's counters.
//!
//! Each device reports its position as the `x`/`y` sources (-32768 left/top..32767), its
//! buttons as evdev key sources (`BTN_LEFT`...), `offscreen` while a gun points off the
//! screen, and a mouse's wheel as `wheel_up` / `wheel_down`, each notch held
//! [`WHEEL_NOTCH`] (one notch a press the game's payload sees: Tapping Skill Test's pinch).
//!
//! With `input.guns_grab` the guns are grabbed: the desktop's pointer does not move, no click
//! reaches it (Batocera's Wayland with a Sinden: its off-screen reload clicked at the screen's
//! edge, the game window lost the focus: Tomb Raider and Rabbids paused, Luigi's Mansion's
//! Direct3D device reset failed, Time Crisis 5's RSLauncher came to the front). The game's
//! input is the launcher's. Mice are not grabbed: the desktop's own mouse.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode, RelativeAxisCode, SynchronizationCode};
use wal_protocol::MAX_PLAYERS;

use crate::mapping::RawKey;

/// Consecutive reports at the edge before a gun counts as off-screen: a Sinden emits isolated
/// 0 / max samples when it briefly loses the border.
const OFFSCREEN_DEBOUNCE: u32 = 3;

/// How long a mouse wheel notch holds its `wheel_up` / `wheel_down` source; notches in a row
/// add up (the input hub samples the sources: a press and its release sent together would be
/// lost).
const WHEEL_NOTCH: std::time::Duration = std::time::Duration::from_millis(64);

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

fn is_tagged_gun(path: &Path) -> bool {
    udev_property(path, "ID_INPUT_GUN").as_deref() == Some("1")
}

/// Without gun tags: an absolute pointer of a real device.
fn is_untagged_gun(path: &Path, dev: &Device) -> bool {
    if udev_property(path, "ID_INPUT_GUN").is_some() || is_virtual(path) {
        return false;
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

/// A uinput device (`/sys/devices/virtual/input/...`).
fn is_virtual(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else { return false };
    std::fs::canonicalize(format!("/sys/class/input/{name}")).is_ok_and(|p| p.starts_with("/sys/devices/virtual"))
}

/// The HID device of an evdev node (`/sys/class/input/eventN/device/device`).
fn hid_device(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    std::fs::canonicalize(format!("/sys/class/input/{name}/device/device")).ok()
}

fn is_touchpad(path: &Path, dev: &Device) -> bool {
    if let Some(v) = udev_property(path, "ID_INPUT_TOUCHPAD") {
        return v == "1";
    }
    dev.supported_absolute_axes().is_some_and(|a| a.contains(AbsoluteAxisCode::ABS_X))
        && dev.supported_keys().is_some_and(|k| k.contains(KeyCode::BTN_TOUCH))
}

/// Finds the guns, then the mice for the remaining players, and starts a reader thread for
/// each one, sending its updates to `tx`.
pub fn start(use_mice: bool, max_mice: usize, mouse_screen: [u32; 2], wrap: bool, grab: bool, tx: Sender<PointerEvent>) -> Vec<Pointer> {
    if wrap {
        return start_trackball(mouse_screen, tx);
    }
    let mut devices: Vec<(PathBuf, Device)> = evdev::enumerate().collect();
    devices.sort_by_key(|(p, _)| event_number(p));
    let touchpads: Vec<PathBuf> =
        devices.iter().filter(|(p, d)| is_touchpad(p, d)).filter_map(|(p, _)| hid_device(p)).collect();
    let tagged = devices.iter().any(|(p, _)| is_tagged_gun(p));
    let is_gun = |p: &Path, d: &Device| if tagged { is_tagged_gun(p) } else { is_untagged_gun(p, d) };
    let (guns, others): (Vec<_>, Vec<_>) = devices.into_iter().partition(|(p, d)| is_gun(p, d));
    let mut mice: Vec<_> = others
        .into_iter()
        // `ID_INPUT_MOUSE=0`: a gun's raw node, not a mouse
        .filter(|(p, d)| {
            use_mice
                && is_mouse(d)
                && udev_property(p, "ID_INPUT_MOUSE").as_deref() != Some("0")
                && !hid_device(p).is_some_and(|h| touchpads.contains(&h))
        })
        .collect();
    // plugged-in (USB) mice before the built-in pointers (touchpad, TrackPoint): with
    // `guns_mice: 1` on a laptop, the external mouse aims
    mice.sort_by_key(|(p, _)| (udev_property(p, "ID_BUS").as_deref() != Some("usb"), event_number(p)));
    let mice = mice.into_iter().take(if max_mice == 0 { usize::MAX } else { max_mice });

    let gun_count = guns.len();
    let mut pointers = Vec::new();
    for (path, dev) in guns.into_iter().chain(mice).take(MAX_PLAYERS) {
        let index = pointers.len();
        let is_gun = index < gun_count;
        let pointer = Pointer { path, name: dev.name().unwrap_or("?").to_string(), is_gun, player: index };
        let tx = tx.clone();
        if is_gun {
            let mut dev = dev;
            if grab && let Err(e) = dev.grab() {
                eprintln!("input: {}: not grabbed ({e}): its clicks also reach the desktop", pointer.path.display());
            }
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

/// `v` in `0..size` -> -32768..=32767 (`size` one past 32767: back to -32768).
fn wrapped(v: i32, size: i32) -> i32 {
    (v as i64 * 65536 / size as i64) as i32 - 32768
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

/// A wheel direction's source held until its notches' time is over (`None`: released).
type WheelHold = Arc<Mutex<Option<std::time::Instant>>>;

/// One notch of wheel direction `dir`: pressed now (released [`WHEEL_NOTCH`] later), or held
/// one notch longer.
fn wheel_notch(index: usize, dir: u8, hold: &WheelHold, tx: &Sender<PointerEvent>) -> Result<(), std::sync::mpsc::SendError<PointerEvent>> {
    use std::time::Instant;
    let mut until = hold.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = until.as_mut() {
        *t = (*t).max(Instant::now()) + WHEEL_NOTCH;
        return Ok(());
    }
    *until = Some(Instant::now() + WHEEL_NOTCH);
    tx.send((index, RawKey::GunWheel(dir), 1))?;
    let (hold, tx) = (hold.clone(), tx.clone());
    std::thread::spawn(move || {
        loop {
            let left = {
                let mut until = hold.lock().unwrap_or_else(|e| e.into_inner());
                match until.map(|t| t.saturating_duration_since(Instant::now())) {
                    Some(left) if !left.is_zero() => left,
                    _ => {
                        *until = None;
                        break;
                    }
                }
            };
            std::thread::sleep(left);
        }
        let _ = tx.send((index, RawKey::GunWheel(dir), 0));
    });
    Ok(())
}

fn read_mouse(index: usize, mut dev: Device, screen: [u32; 2], tx: Sender<PointerEvent>) {
    let size = screen.map(|s| s.max(1) as i32);
    // starts mid-screen
    let mut pos = size.map(|s| s / 2);
    let wheel: [WheelHold; 2] = Default::default();
    while let Ok(events) = dev.fetch_events() {
        for ev in events {
            let sent = match ev.destructure() {
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, value) if value != 0 => {
                    let dir = (value < 0) as u8;
                    (0..value.unsigned_abs()).try_for_each(|_| wheel_notch(index, dir, &wheel[dir as usize], &tx))
                }
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

/// `input.mouse_wrap`: the mice and touchpads roll one trackball, pointer 0 (player 1).
fn start_trackball(screen: [u32; 2], tx: Sender<PointerEvent>) -> Vec<Pointer> {
    let mut devices: Vec<(PathBuf, Device)> = evdev::enumerate().collect();
    devices.sort_by_key(|(p, _)| event_number(p));
    let touchpads: Vec<PathBuf> =
        devices.iter().filter(|(p, d)| is_touchpad(p, d)).filter_map(|(p, _)| hid_device(p)).collect();
    let size = screen.map(|s| s.max(1) as i32);
    let position = Arc::new(Mutex::new(size.map(|s| s / 2)));
    let mut names = Vec::new();
    let mut first = None;
    for (path, dev) in devices {
        let touchpad = is_touchpad(&path, &dev);
        // a touchpad's own mouse node never moves, a gun's raw node is no mouse
        let mouse = is_mouse(&dev)
            && udev_property(&path, "ID_INPUT_MOUSE").as_deref() != Some("0")
            && !hid_device(&path).is_some_and(|h| touchpads.contains(&h));
        if !touchpad && !mouse {
            continue;
        }
        names.push(dev.name().unwrap_or("?").to_string());
        first.get_or_insert(path);
        let (position, tx) = (position.clone(), tx.clone());
        std::thread::spawn(move || read_rolling(dev, touchpad, size, position, tx));
    }
    first.map(|path| Pointer { path, name: names.join(", "), is_gun: false, player: 0 }).into_iter().collect()
}

/// A mouse's motion, or a touchpad finger's, rolling the shared trackball `position`.
fn read_rolling(mut dev: Device, touchpad: bool, size: [i32; 2], position: Arc<Mutex<[i32; 2]>>, tx: Sender<PointerEvent>) {
    // the finger's last position (none until it touches)
    let mut finger: [Option<i32>; 2] = [None; 2];
    let roll = |axis: usize, motion: i32| {
        let mut pos = position.lock().unwrap_or_else(|e| e.into_inner());
        pos[axis] = (pos[axis] + motion).rem_euclid(size[axis]);
        tx.send((0, RawKey::GunAxis(axis as u8), wrapped(pos[axis], size[axis])))
    };
    while let Ok(events) = dev.fetch_events() {
        for ev in events {
            let sent = match ev.destructure() {
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, value) if !touchpad => roll(0, value),
                EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, value) if !touchpad => roll(1, value),
                EventSummary::AbsoluteAxis(_, code @ (AbsoluteAxisCode::ABS_X | AbsoluteAxisCode::ABS_Y), value) if touchpad => {
                    let axis = (code == AbsoluteAxisCode::ABS_Y) as usize;
                    match finger[axis].replace(value) {
                        Some(last) => roll(axis, value - last),
                        None => continue,
                    }
                }
                EventSummary::Key(_, KeyCode::BTN_TOUCH, _) => {
                    finger = [None; 2];
                    continue;
                }
                EventSummary::Key(_, code, value) if value != 2 => tx.send((0, RawKey::Key(code.code()), value)),
                _ => continue,
            };
            if sent.is_err() {
                return;
            }
        }
    }
    eprintln!("input: trackball device '{}' disconnected", dev.name().unwrap_or("?"));
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

    #[test]
    fn wraps() {
        assert_eq!(wrapped(0, 65536), -32768);
        assert_eq!(wrapped(65535, 65536), 32767);
        assert_eq!(wrapped(960, 1920), 0);
    }
}
