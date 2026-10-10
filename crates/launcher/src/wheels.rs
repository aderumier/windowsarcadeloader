//! Wheel calibrations: a wheel (or pedals) calibrated in the GUI is read as a gamepad, so every
//! game's gamepad mapping drives it with no per-game wheel table.
//!
//! Machine-wide, by device name (`userprofiles/wheels.yaml`). The calibration gives the axes'
//! ends and the gamepad buttons of the wheel's buttons. Values are normalized to the gamepad's
//! (the reference of the games' calibration test menus, set up with gamepads):
//!
//! * steering -> `leftx`: full left -32768, center 0, full right 32767;
//! * gas -> `r2`, brake -> `l2`: released 0, fully pressed 32767 (the triggers' range);
//! * clutch -> the right stick's lower half (`+righty`): released 0, pressed 32767.
//!
//! Each end has a small band (3 % of its half) that gives exactly its value: a released pedal
//! is 0 and a fully turned wheel is the stick's end even with a jittering sensor.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sdl3::gamepad::Axis as PadAxis;
use serde::{Deserialize, Serialize};

use crate::config::USER_PROFILES;
use crate::mapping::{self, RawKey};

const FILE: &str = "wheels.yaml";

/// Band at each end of an axis giving exactly that end (per mille of the half range).
const END_BAND: i64 = 30;

/// The steering axis: raw values at full left, center and full right (left may be above right).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Steering {
    pub axis: u8,
    pub left: i32,
    pub center: i32,
    pub right: i32,
}

/// A pedal axis: raw values released and fully pressed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pedal {
    pub axis: u8,
    pub rest: i32,
    pub full: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering: Option<Steering>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gas: Option<Pedal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brake: Option<Pedal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutch: Option<Pedal>,
    /// Gamepad button name (`a`, `r1`, `dpup`...) -> the wheel's raw source (`button4`, `hat0up`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buttons: BTreeMap<String, String>,
}

/// Calibrations by device name.
pub type Wheels = BTreeMap<String, Calibration>;

fn path(root: &Path) -> PathBuf {
    root.join(USER_PROFILES).join(FILE)
}

pub fn load(root: &Path) -> Result<Wheels> {
    let path = path(root);
    if !path.exists() {
        return Ok(Wheels::new());
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let wheels: Option<Wheels> = serde_yaml_ng::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(wheels.unwrap_or_default())
}

pub fn save(root: &Path, wheels: &Wheels) -> Result<()> {
    let path = path(root);
    std::fs::create_dir_all(root.join(USER_PROFILES))?;
    let text = serde_yaml_ng::to_string(wheels)?;
    std::fs::write(&path, format!("# Wheel calibrations (launcher GUI), by device name: read as gamepads\n{text}"))
        .with_context(|| format!("writing {}", path.display()))
}

/// The calibration of a device (its SDL joystick name).
pub fn find<'a>(wheels: &'a Wheels, name: &str) -> Option<&'a Calibration> {
    wheels.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| c)
}

/// `v` from `from` (0) to `to` (32767 or 32768), the end bands giving exactly 0 and the end.
fn scale(v: i32, from: i32, to: i32, end: i64) -> i64 {
    let span = (to - from) as i64;
    if span == 0 {
        return 0;
    }
    // per mille of the way, then the bands
    let t = ((v - from) as i64 * 1000 / span).clamp(0, 1000);
    let t = ((t - END_BAND) * 1000 / (1000 - 2 * END_BAND)).clamp(0, 1000);
    t * end / 1000
}

impl Steering {
    /// -32768 (full left)..=32767 (full right).
    pub fn value(&self, v: i32) -> i32 {
        let toward_right = (v - self.center).signum() == (self.right - self.center).signum();
        if toward_right {
            scale(v, self.center, self.right, 32767) as i32
        } else {
            -(scale(v, self.center, self.left, 32768) as i32)
        }
    }
}

impl Pedal {
    /// 0 (released)..=32767 (fully pressed).
    pub fn value(&self, v: i32) -> i32 {
        scale(v, self.rest, self.full, 32767) as i32
    }
}

/// The gamepad buttons a wheel button can give, in the GUI calibration's order: (name, label).
pub const BUTTONS: [(&str, &str); 14] = [
    ("r1", "RB (R1): right paddle / shift up"),
    ("l1", "LB (L1): left paddle / shift down"),
    ("a", "A"),
    ("b", "B"),
    ("x", "X"),
    ("y", "Y"),
    ("start", "Start"),
    ("back", "View (Back): coin"),
    ("l3", "L3: service"),
    ("r3", "R3: test"),
    ("dpup", "D-pad up"),
    ("dpdown", "D-pad down"),
    ("dpleft", "D-pad left"),
    ("dpright", "D-pad right"),
];

impl Calibration {
    /// The gamepad axes a raw axis drives: (axis, its source name, value).
    fn axes_of(&self, index: u8, value: i32) -> Vec<(PadAxis, &'static str, i32)> {
        let mut out = Vec::new();
        if let Some(s) = self.steering.filter(|s| s.axis == index) {
            out.push((PadAxis::LeftX, "leftx", s.value(value)));
        }
        // gas and brake may share one axis (one half each)
        let pedals = [(self.gas, PadAxis::TriggerRight, "r2"), (self.brake, PadAxis::TriggerLeft, "l2"), (self.clutch, PadAxis::RightY, "righty")];
        for (pedal, axis, name) in pedals {
            if let Some(p) = pedal.filter(|p| p.axis == index) {
                out.push((axis, name, p.value(value)));
            }
        }
        out
    }

    /// The gamepad axes a raw axis drives: (raw key, value).
    pub fn axis(&self, index: u8, value: i32) -> Vec<(RawKey, i32)> {
        self.axes_of(index, value).into_iter().map(|(a, _, v)| (RawKey::PadAxis(a as i32), v)).collect()
    }

    /// The gamepad buttons of a raw button or hat: (name, pressed).
    pub fn button_names(&self, raw: RawKey, value: i32) -> Vec<(&'static str, bool)> {
        let mut out = Vec::new();
        for (name, _) in BUTTONS {
            let Some(source) = self.buttons.get(name).and_then(|s| mapping::parse_joy_source(s)) else { continue };
            if source.key() == raw {
                out.push((name, source.pressed(value)));
            }
        }
        out
    }

    /// The gamepad buttons of a raw button or hat: (raw key, pressed).
    pub fn buttons_of(&self, raw: RawKey, value: i32) -> Vec<(RawKey, i32)> {
        self.button_names(raw, value)
            .into_iter()
            .filter_map(|(name, down)| Some((mapping::parse_pad_source(name)?.key(), down as i32)))
            .collect()
    }

    /// The gamepad inputs of a raw input by source name (`leftx`, `r2`, `a`...), for the GUI's
    /// mapping capture: (name, value; buttons 0/1).
    pub fn named(&self, raw: RawKey, value: i32) -> Vec<(&'static str, i32)> {
        match raw {
            RawKey::JoyAxis(i) => self.axes_of(i, value).into_iter().map(|(_, n, v)| (n, v)).collect(),
            _ => self.button_names(raw, value).into_iter().map(|(n, d)| (n, d as i32)).collect(),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use sdl3::gamepad::Button as PadButton;

    #[test]
    fn steering_ends_and_center() {
        let s = Steering { axis: 0, left: -32768, center: 0, right: 32767 };
        assert_eq!(s.value(-32768), -32768);
        assert_eq!(s.value(-32000), -32768);
        assert_eq!(s.value(0), 0);
        assert_eq!(s.value(500), 0);
        assert_eq!(s.value(32767), 32767);
        assert_eq!(s.value(32000), 32767);
        assert!((s.value(16384) - 16384).abs() < 400);
        // a 270 degree lock on a 900 degree wheel, the axis reversed
        let s = Steering { axis: 0, left: 10000, center: 0, right: -10000 };
        assert_eq!(s.value(10000), -32768);
        assert_eq!(s.value(-12000), 32767);
        assert!(s.value(-5000) > 15000 && s.value(-5000) < 17500);
    }

    #[test]
    fn pedals_like_triggers() {
        // released at +32767, pressed toward -32768 (inverted, as most wheels)
        let p = Pedal { axis: 2, rest: 32767, full: -32768 };
        assert_eq!(p.value(32767), 0);
        assert_eq!(p.value(31000), 0);
        assert_eq!(p.value(-32768), 32767);
        assert_eq!(p.value(-31500), 32767);
        assert!((p.value(0) - 16384).abs() < 400);
        // gas and brake on the two halves of one axis
        let c = Calibration {
            gas: Some(Pedal { axis: 1, rest: 0, full: -32768 }),
            brake: Some(Pedal { axis: 1, rest: 0, full: 32767 }),
            ..Default::default()
        };
        let gas = RawKey::PadAxis(PadAxis::TriggerRight as i32);
        let brake = RawKey::PadAxis(PadAxis::TriggerLeft as i32);
        assert_eq!(c.axis(1, -32768), vec![(gas, 32767), (brake, 0)]);
        assert_eq!(c.axis(1, 32767), vec![(gas, 0), (brake, 32767)]);
    }

    #[test]
    fn buttons_and_hats() {
        let mut c = Calibration::default();
        c.buttons.insert("r1".into(), "button4".into());
        c.buttons.insert("dpup".into(), "hat0up".into());
        let r1 = RawKey::PadButton(PadButton::RightShoulder as i32);
        let up = RawKey::PadButton(PadButton::DPadUp as i32);
        assert_eq!(c.buttons_of(RawKey::JoyButton(4), 1), vec![(r1, 1)]);
        assert_eq!(c.buttons_of(RawKey::JoyHat(0), 0x09), vec![(up, 1)]);
        assert_eq!(c.buttons_of(RawKey::JoyHat(0), 0x04), vec![(up, 0)]);
        assert!(c.buttons_of(RawKey::JoyButton(1), 1).is_empty());
    }
}
