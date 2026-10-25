//! Players of the SDL devices (gamepads, wheels): which one is P1, P2...
//!
//! By default the frontend's order: Batocera's generator passes its players' evdev nodes in
//! `WAL_PLAYER_PADS` (`;` separated, P1 first), the devices it does not list follow in SDL's
//! order. A priority list set in the GUI (`userprofiles/players.yaml`, machine-wide, by SDL GUID)
//! goes first: its connected devices take P1, P2... in its order, the others follow. Devices
//! sharing a GUID (two identical pads) take that GUID's entries in the frontend's order.
//!
//! A wheel is always P1 (a second wheel takes the first free player), and the first gamepad
//! joins it there: the wheel's player navigates the menus and presses the cabinet buttons with
//! it. The other devices take the first free player.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use wal_protocol::MAX_PLAYERS;

use crate::config::USER_PROFILES;

const FILE: &str = "players.yaml";

/// The frontend's player order: evdev device nodes, P1 first.
pub const ORDER_ENV: &str = "WAL_PLAYER_PADS";

/// An entry of the priority list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    /// SDL GUID: what the entry matches.
    pub guid: String,
    /// The device's name when added (shown in the GUI).
    pub name: String,
}

/// The priority list, P1 first.
pub type Priority = Vec<Pad>;

/// A connected device, for the ordering.
pub struct Device<T> {
    pub key: T,
    pub guid: String,
    /// Its evdev node.
    pub path: Option<String>,
}

fn path(root: &Path) -> PathBuf {
    root.join(USER_PROFILES).join(FILE)
}

pub fn load(root: &Path) -> Result<Priority> {
    let path = path(root);
    if !path.exists() {
        return Ok(Priority::new());
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let list: Option<Priority> = serde_yaml_ng::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(list.unwrap_or_default())
}

pub fn save(root: &Path, list: &Priority) -> Result<()> {
    let path = path(root);
    if list.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
        return Ok(());
    }
    std::fs::create_dir_all(root.join(USER_PROFILES))?;
    let text = serde_yaml_ng::to_string(list)?;
    std::fs::write(&path, format!("# Player priority (launcher GUI): P1 first, by SDL GUID\n{text}"))
        .with_context(|| format!("writing {}", path.display()))
}

/// The frontend's order (`WAL_PLAYER_PADS`).
pub fn frontend_order() -> Vec<String> {
    std::env::var(ORDER_ENV)
        .map(|v| v.split(';').filter(|p| !p.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// `devices` (in SDL's order) in player order: the priority list's, then the frontend's.
pub fn order<T: Copy>(devices: &[Device<T>], priority: &[Pad], frontend: &[String]) -> Vec<T> {
    let rank = |d: &Device<T>| d.path.as_ref().and_then(|p| frontend.iter().position(|f| f == p)).unwrap_or(usize::MAX);
    let mut base: Vec<&Device<T>> = devices.iter().collect();
    // stable: unlisted devices keep SDL's order
    base.sort_by_key(|d| rank(d));
    let mut out = Vec::with_capacity(base.len());
    for pad in priority {
        if let Some(i) = base.iter().position(|d| d.guid.eq_ignore_ascii_case(&pad.guid)) {
            out.push(base.remove(i).key);
        }
    }
    out.extend(base.iter().map(|d| d.key));
    out
}

/// The player of a device added after `taken` (the players of the devices there, and whether
/// each is a wheel): P1 for a wheel, or a gamepad, while P1 has none of its kind and the other
/// kind is there or not, else the first free one.
pub fn next_player(wheel: bool, taken: &[(usize, bool)]) -> usize {
    let on_p1 = |kind: bool| taken.iter().any(|&(p, w)| p == 0 && w == kind);
    if !on_p1(wheel) && (wheel || on_p1(true)) {
        return 0;
    }
    (0..MAX_PLAYERS).find(|p| !taken.iter().any(|(t, _)| t == p)).unwrap_or(MAX_PLAYERS)
}

/// Whether SDL knows a device as a wheel (SDL initialized).
pub fn sdl_is_wheel(id: u32) -> bool {
    let t = unsafe { sdl3::sys::joystick::SDL_GetJoystickTypeForID(sdl3::sys::joystick::SDL_JoystickID(id)) };
    t == sdl3::sys::joystick::SDL_JOYSTICK_TYPE_WHEEL
}

/// The SDL GUID of a device (SDL initialized).
pub fn sdl_guid(id: u32) -> String {
    let guid = unsafe { sdl3::sys::joystick::SDL_GetJoystickGUIDForID(sdl3::sys::joystick::SDL_JoystickID(id)) };
    let mut buf = [0 as std::ffi::c_char; 33];
    unsafe {
        sdl3::sys::guid::SDL_GUIDToString(guid, buf.as_mut_ptr(), buf.len() as i32);
        std::ffi::CStr::from_ptr(buf.as_ptr())
    }
    .to_string_lossy()
    .into_owned()
}

/// The device node of a device (`/dev/input/eventN` on Linux).
pub fn sdl_path(id: u32) -> Option<String> {
    let p = unsafe { sdl3::sys::joystick::SDL_GetJoystickPathForID(sdl3::sys::joystick::SDL_JoystickID(id)) };
    (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(key: u32, guid: &str, path: &str) -> Device<u32> {
        Device { key, guid: guid.into(), path: Some(path.into()) }
    }

    fn pad(guid: &str) -> Pad {
        Pad { guid: guid.into(), name: String::new() }
    }

    #[test]
    fn frontend_then_priority() {
        let devices = [dev(1, "aa", "/e1"), dev(2, "bb", "/e2"), dev(3, "cc", "/e3")];
        let frontend = ["/e3".to_string(), "/e1".to_string()];
        // no list: the frontend's order, unlisted devices last
        assert_eq!(order(&devices, &[], &frontend), [3, 1, 2]);
        assert_eq!(order(&devices, &[], &[]), [1, 2, 3]);
        // the list first, then the frontend's order; entries not connected skipped
        assert_eq!(order(&devices, &[pad("bb"), pad("zz")], &frontend), [2, 3, 1]);
        assert_eq!(order(&devices, &[pad("BB"), pad("aa")], &frontend), [2, 1, 3]);
    }

    #[test]
    fn gamepad_joins_wheel_on_p1() {
        // wheel P1, then a pad joins it, the next pad P2
        assert_eq!(next_player(true, &[]), 0);
        assert_eq!(next_player(false, &[(0, true)]), 0);
        assert_eq!(next_player(false, &[(0, true), (0, false)]), 1);
        // pads first: the wheel joins P1 all the same, the next pad P3
        assert_eq!(next_player(false, &[]), 0);
        assert_eq!(next_player(false, &[(0, false)]), 1);
        assert_eq!(next_player(true, &[(0, false), (1, false)]), 0);
        assert_eq!(next_player(false, &[(0, false), (1, false), (0, true)]), 2);
        // a second wheel does not share P1
        assert_eq!(next_player(true, &[(0, true)]), 1);
    }

    #[test]
    fn same_guid_by_frontend_order() {
        let devices = [dev(1, "aa", "/e1"), dev(2, "bb", "/e2"), dev(3, "aa", "/e3")];
        let frontend = ["/e3".to_string(), "/e2".to_string(), "/e1".to_string()];
        assert_eq!(order(&devices, &[pad("aa")], &frontend), [3, 2, 1]);
        assert_eq!(order(&devices, &[pad("aa"), pad("aa")], &frontend), [3, 1, 2]);
        assert_eq!(order(&devices, &[pad("aa"), pad("aa")], &[]), [1, 3, 2]);
    }
}
