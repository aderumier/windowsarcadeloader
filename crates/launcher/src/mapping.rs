//! Physical inputs -> virtual arcade sticks.
//!
//! A mapping table associates source names (`x`, `l2`, `+axis2`, `hat0up`, `KEY_Z`...) with
//! virtual stick targets (`b1`..`b8`, `up`, `start`, `lx`, `-accel`...). A leading `-`
//! on an axis target inverts it. `none` disables a default entry.
//!
//! Conversions: digital -> axis gives a full deflection, analog -> button presses past
//! the deadzone, a full range axis -> pedal is rescaled from -32768..32767 to 0..32767.

use std::collections::HashMap;

use anyhow::{Result, bail};
use sdl3::gamepad::{Axis as PadAxis, Button as PadButton};
use wal_protocol::{Axis, InputFrame, MAX_PLAYERS, StickState, button};

use crate::config::MapTable;

/// One physical input whose state is tracked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawKey {
    PadButton(i32),
    PadAxis(i32),
    JoyButton(u8),
    JoyAxis(u8),
    JoyHat(u8),
    /// evdev keys and buttons (keyboards, gun and mouse buttons).
    Key(u16),
    /// Gun/mouse position: 0 = x, 1 = y (-32768 left/top..=32767).
    GunAxis(u8),
    /// Gun pointing off the screen (0/1).
    GunOffscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Range {
    /// Digital (button, key, hat direction mask).
    Digital(u8),
    /// -32768..=32767.
    Full,
    /// Only the positive (or negated negative) half, 0..=32767.
    Positive,
    Negative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    key: RawKey,
    range: Range,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Button(u32),
    Axis { axis: Axis, invert: bool },
}

pub type Mapping = Vec<(Source, Target)>;

fn pad_button(name: &str) -> Option<PadButton> {
    use PadButton::*;
    Some(match name {
        "a" | "south" => South,
        "b" | "east" => East,
        "x" | "west" => West,
        "y" | "north" => North,
        "back" | "select" => Back,
        "guide" | "home" => Guide,
        "start" => Start,
        "l3" | "leftstick" => LeftStick,
        "r3" | "rightstick" => RightStick,
        "l1" | "lb" | "leftshoulder" => LeftShoulder,
        "r1" | "rb" | "rightshoulder" => RightShoulder,
        "dpup" => DPadUp,
        "dpdown" => DPadDown,
        "dpleft" => DPadLeft,
        "dpright" => DPadRight,
        "misc1" => Misc1,
        "paddle1" => RightPaddle1,
        "paddle2" => LeftPaddle1,
        "paddle3" => RightPaddle2,
        "paddle4" => LeftPaddle2,
        "touchpad" => Touchpad,
        _ => return None,
    })
}

fn pad_axis(name: &str) -> Option<(PadAxis, bool)> {
    use PadAxis::*;
    // (axis, is a trigger: 0..32767)
    Some(match name {
        "leftx" => (LeftX, false),
        "lefty" => (LeftY, false),
        "rightx" => (RightX, false),
        "righty" => (RightY, false),
        "l2" | "lt" | "lefttrigger" => (TriggerLeft, true),
        "r2" | "rt" | "righttrigger" => (TriggerRight, true),
        _ => return None,
    })
}

fn split_sign(name: &str) -> (Option<char>, &str) {
    match name.chars().next() {
        Some(c @ ('+' | '-')) => (Some(c), &name[1..]),
        _ => (None, name),
    }
}

fn half(sign: Option<char>) -> Range {
    match sign {
        Some('+') => Range::Positive,
        Some('-') => Range::Negative,
        _ => Range::Full,
    }
}

fn index(s: &str, prefix: &str) -> Option<u8> {
    s.strip_prefix(prefix)?.parse().ok()
}

/// Gamepad (SDL positional) source names, with `+`/`-` half axes.
pub fn parse_pad_source(name: &str) -> Option<Source> {
    let lower = name.to_ascii_lowercase();
    let (sign, base) = split_sign(&lower);
    if let Some(b) = pad_button(base).filter(|_| sign.is_none()) {
        return Some(Source { key: RawKey::PadButton(b as i32), range: Range::Digital(1) });
    }
    let (axis, trigger) = pad_axis(base)?;
    let range = if trigger { Range::Positive } else { half(sign) };
    Some(Source { key: RawKey::PadAxis(axis as i32), range })
}

/// Raw joystick source names: `button3`, `axis1`, `+axis2`, `-axis2`, `hat0up`.
pub fn parse_joy_source(name: &str) -> Option<Source> {
    let lower = name.to_ascii_lowercase();
    let (sign, base) = split_sign(&lower);
    if let Some(i) = index(base, "button").filter(|_| sign.is_none()) {
        return Some(Source { key: RawKey::JoyButton(i), range: Range::Digital(1) });
    }
    if let Some(i) = index(base, "axis") {
        return Some(Source { key: RawKey::JoyAxis(i), range: half(sign) });
    }
    let hat = base.strip_prefix("hat")?;
    let digits = hat.chars().take_while(|c| c.is_ascii_digit()).count();
    let i = hat[..digits].parse().ok()?;
    let mask = match &hat[digits..] {
        "up" => 0x01,
        "right" => 0x02,
        "down" => 0x04,
        "left" => 0x08,
        _ => return None,
    };
    Some(Source { key: RawKey::JoyHat(i), range: Range::Digital(mask) })
}

/// Keyboard source names: evdev key names (`KEY_Z`).
pub fn parse_key_source(name: &str) -> Option<Source> {
    let code: evdev::KeyCode = name.to_ascii_uppercase().parse().ok()?;
    Some(Source { key: RawKey::Key(code.code()), range: Range::Digital(1) })
}

/// Gun and mouse source names: `x`, `y` (position), `offscreen`, evdev buttons (`BTN_LEFT`).
pub fn parse_gun_source(name: &str) -> Option<Source> {
    Some(match name.to_ascii_lowercase().as_str() {
        "x" => Source { key: RawKey::GunAxis(0), range: Range::Full },
        "y" => Source { key: RawKey::GunAxis(1), range: Range::Full },
        "offscreen" => Source { key: RawKey::GunOffscreen, range: Range::Digital(1) },
        _ => return parse_key_source(name),
    })
}

pub fn parse_target(name: &str) -> Option<Target> {
    if let Some(b) = button::from_name(name) {
        return Some(Target::Button(b));
    }
    let (sign, base) = split_sign(name);
    Axis::from_name(base).map(|axis| Target::Axis { axis, invert: sign == Some('-') })
}

/// The gamepad's left stick as its d-pad: when the four d-pad buttons are on the four
/// directions the game uses, each stick axis that drives nothing the game uses (a free axis,
/// unmapped or on an unused one) gets its halves on the same directions. The entries to add.
pub fn stick_dpad(table: &MapTable, uses: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    let get = |src: &str| table.iter().find(|(s, _)| s.eq_ignore_ascii_case(src)).map(|(_, d)| d.as_str());
    let dpad = [("dpup", "up"), ("dpdown", "down"), ("dpleft", "left"), ("dpright", "right")];
    if !dpad.iter().all(|(b, d)| get(b).is_some_and(|t| t.eq_ignore_ascii_case(d)) && uses(d)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (axis, neg, pos) in [("leftx", "left", "right"), ("lefty", "up", "down")] {
        let (minus, plus) = (format!("-{axis}"), format!("+{axis}"));
        // a half mapped (or set to none) by the user: theirs
        if get(&minus).is_some() || get(&plus).is_some() {
            continue;
        }
        let free = match get(axis).filter(|d| !d.eq_ignore_ascii_case("none")).and_then(parse_target) {
            None => true,
            Some(Target::Axis { axis, .. }) => !uses(axis.name()),
            Some(Target::Button(_)) => false,
        };
        if free {
            out.push((minus, neg.to_string()));
            out.push((plus, pos.to_string()));
        }
    }
    out
}

/// Compiles a table; `parse` is the source parser of the device kind.
pub fn compile(table: &MapTable, parse: fn(&str) -> Option<Source>) -> Result<Mapping> {
    let mut out = Vec::new();
    for (src, dst) in table {
        if dst.eq_ignore_ascii_case("none") {
            continue;
        }
        let Some(s) = parse(src) else { bail!("unknown input '{src}'") };
        let Some(t) = parse_target(dst) else { bail!("unknown virtual stick input '{dst}' (for '{src}')") };
        out.push((s, t));
    }
    Ok(out)
}

/// Physical state of one device and its contribution to a player's stick.
pub struct DeviceState {
    pub player: usize,
    pub mapping: Mapping,
    pub raw: HashMap<RawKey, i32>,
}

enum Value {
    Digital(bool),
    /// value, and whether it only covers 0..=32767
    Analog(i32, bool),
}

impl DeviceState {
    pub fn new(player: usize, mapping: Mapping) -> Self {
        DeviceState { player, mapping, raw: HashMap::new() }
    }

    fn value(&self, s: &Source) -> Value {
        let raw = self.raw.get(&s.key).copied().unwrap_or(0);
        match s.range {
            Range::Digital(mask) => Value::Digital(raw as u8 & mask != 0),
            Range::Full => Value::Analog(raw, false),
            Range::Positive => Value::Analog(raw.max(0), true),
            Range::Negative => Value::Analog((-raw - 1).max(0), true),
        }
    }

    /// True when every source is pressed (for the exit combo).
    pub fn all_pressed(&self, sources: &[Source]) -> bool {
        !sources.is_empty() && sources.iter().all(|s| matches!(self.value(s), Value::Digital(true)))
    }

    pub fn contribution(&self, deadzone: i16) -> StickState {
        let mut st = StickState::default();
        for (src, tgt) in &self.mapping {
            match (self.value(src), *tgt) {
                (Value::Digital(p), Target::Button(b)) => {
                    if p {
                        st.buttons |= b
                    }
                }
                (Value::Analog(v, _), Target::Button(b)) => {
                    if v > deadzone as i32 {
                        st.buttons |= b
                    }
                }
                (value, Target::Axis { axis, invert }) => {
                    let v = axis_value(value, axis.is_pedal(), invert);
                    let slot = &mut st.axes[axis as usize];
                    if v.unsigned_abs() > slot.unsigned_abs() {
                        *slot = v;
                    }
                }
            }
        }
        st
    }
}

fn axis_value(value: Value, pedal: bool, invert: bool) -> i16 {
    let v = match (value, pedal) {
        (Value::Digital(false), _) => return 0,
        (Value::Digital(true), true) => 32767,
        (Value::Digital(true), false) => {
            if invert {
                -32768
            } else {
                32767
            }
        }
        (Value::Analog(v, half), true) => {
            let v = if half { v } else { (v + 32768) / 2 };
            if invert { 32767 - v } else { v }
        }
        (Value::Analog(v, _), false) => {
            if invert {
                -v - 1
            } else {
                v
            }
        }
    };
    v.clamp(-32768, 32767) as i16
}

/// Merges device contributions into the virtual sticks.
pub fn merge<'a>(devices: impl Iterator<Item = &'a DeviceState>, deadzone: i16, stick_as_dpad: bool) -> InputFrame {
    let mut frame = InputFrame::default();
    for dev in devices {
        if dev.player >= MAX_PLAYERS {
            continue;
        }
        let c = dev.contribution(deadzone);
        let p = &mut frame.players[dev.player];
        p.buttons |= c.buttons;
        for (slot, v) in p.axes.iter_mut().zip(c.axes) {
            if v.unsigned_abs() > slot.unsigned_abs() {
                *slot = v;
            }
        }
    }
    if stick_as_dpad {
        for p in &mut frame.players {
            let (x, y) = (p.axis(Axis::LeftX), p.axis(Axis::LeftY));
            let dz = deadzone.max(1);
            if x < -dz {
                p.buttons |= button::LEFT;
            }
            if x > dz {
                p.buttons |= button::RIGHT;
            }
            if y < -dz {
                p.buttons |= button::UP;
            }
            if y > dz {
                p.buttons |= button::DOWN;
            }
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gamepad_compiles() {
        let table: MapTable = [("x", "b1"), ("l2", "b8"), ("a", "b5")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let m = compile(&table, parse_pad_source).unwrap();
        let mut dev = DeviceState::new(0, m);
        dev.raw.insert(RawKey::PadButton(PadButton::West as i32), 1);
        dev.raw.insert(RawKey::PadAxis(PadAxis::TriggerLeft as i32), 30000);
        let c = dev.contribution(8000);
        assert_eq!(c.buttons, button::B1 | button::B8);
    }

    #[test]
    fn pedals_and_hats() {
        let table: MapTable = [("axis2", "-accel"), ("-axis1", "brake"), ("hat0left", "left"), ("axis0", "lx")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let mut dev = DeviceState::new(1, compile(&table, parse_joy_source).unwrap());
        dev.raw.insert(RawKey::JoyAxis(2), 32767); // released, inverted pedal
        dev.raw.insert(RawKey::JoyAxis(1), -32768); // fully pressed half axis
        dev.raw.insert(RawKey::JoyHat(0), 0x08 | 0x01);
        dev.raw.insert(RawKey::JoyAxis(0), -20000);
        let frame = merge([&dev].into_iter(), 8000, true);
        let p = frame.players[1];
        assert_eq!(p.axis(Axis::Accel), 0);
        assert_eq!(p.axis(Axis::Brake), 32767);
        assert_eq!(p.axis(Axis::LeftX), -20000);
        assert_eq!(p.buttons, button::LEFT);
    }

    #[test]
    fn keyboard_and_targets() {
        assert!(parse_key_source("KEY_LEFTCTRL").is_some());
        assert!(parse_key_source("key_z").is_some());
        assert!(parse_key_source("KEY_NOPE").is_none());
        assert_eq!(parse_target("-ly"), Some(Target::Axis { axis: Axis::LeftY, invert: true }));
        assert_eq!(parse_target("b8"), Some(Target::Button(button::B8)));
        assert!(parse_target("b9").is_none());
    }

    #[test]
    fn stick_follows_dpad() {
        let t = |e: &[(&str, &str)]| -> MapTable { e.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() };
        let dpad = [("dpup", "up"), ("dpdown", "down"), ("dpleft", "left"), ("dpright", "right")];
        let mut table = t(&dpad);
        table.insert("leftx".into(), "lx".into());
        let all = |_: &str| true;
        let no_axes = |c: &str| !["lx", "ly"].contains(&c);
        // the wheel (lx used): x stays, y (unmapped) follows the d-pad
        assert_eq!(stick_dpad(&table, all), vec![("-lefty".into(), "up".into()), ("+lefty".into(), "down".into())]);
        assert_eq!(stick_dpad(&table, no_axes).len(), 4);
        // directions the game does not use, or a half the user mapped
        assert!(stick_dpad(&table, |c| no_axes(c) && c != "up").is_empty());
        table.insert("+lefty".into(), "b1".into());
        assert_eq!(stick_dpad(&table, no_axes).len(), 2);
        table.remove("dpleft");
        assert!(stick_dpad(&table, no_axes).is_empty());
    }

    #[test]
    fn gun_sources() {
        let table: MapTable = [("x", "lx"), ("y", "ly"), ("BTN_LEFT", "b1"), ("offscreen", "b4")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let mut dev = DeviceState::new(0, compile(&table, parse_gun_source).unwrap());
        dev.raw.insert(RawKey::GunAxis(0), -32768);
        dev.raw.insert(RawKey::GunAxis(1), 12345);
        dev.raw.insert(RawKey::Key(evdev::KeyCode::BTN_LEFT.code()), 1);
        dev.raw.insert(RawKey::GunOffscreen, 1);
        let c = dev.contribution(8000);
        assert_eq!(c.axis(Axis::LeftX), -32768);
        assert_eq!(c.axis(Axis::LeftY), 12345);
        assert_eq!(c.buttons, button::B1 | button::B4);
        assert!(parse_gun_source("BTN_MIDDLE").is_some());
    }
}
