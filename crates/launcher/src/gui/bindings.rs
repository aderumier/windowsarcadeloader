//! Mapping tables as the GUI shows and edits them: the game's controls (rows), the physical
//! inputs bound to each, a captured input turned into a `source: target` entry, and the user
//! layer (what differs from the system profile).

use serde_yaml_ng::{Mapping, Value};
use wal_protocol::Axis;

use crate::config::{MapTable, Profile};
use crate::mapping::{self, Source, Target};

/// The kind of device a table is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Keyboard,
    Gamepad,
    /// Raw joysticks (`input.devices`): wheels, pedals, shifters.
    Wheel,
}

impl Device {
    pub fn parse(self, name: &str) -> Option<Source> {
        match self {
            Device::Keyboard => mapping::parse_key_source(name),
            Device::Gamepad => mapping::parse_pad_source(name),
            Device::Wheel => mapping::parse_joy_source(name),
        }
    }
}

/// How a row's inputs drive its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A button (digital target).
    Button,
    /// A pedal (accel, brake): a trigger, a pedal axis or a key.
    Pedal,
    /// A stick or wheel axis, from a whole analog axis.
    Analog,
    /// One direction of a stick axis (left / up), from a key, a button or half an axis.
    Negative,
    /// The other one (right / down).
    Positive,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Virtual stick input: `b1`, `start`, `lx`, `accel`...
    pub target: String,
    pub kind: RowKind,
    pub label: String,
}

impl Row {
    /// Target name of an entry made for this row.
    fn dst(&self, invert: bool) -> String {
        let neg = matches!(self.kind, RowKind::Negative) != invert;
        if neg { format!("-{}", self.target) } else { self.target.clone() }
    }
}

/// Virtual stick inputs, in the order the GUI lists them.
const ORDER: [&str; 23] = [
    "up", "down", "left", "right", "b1", "b2", "b3", "b4", "b5", "b6", "b7", "b8", "lx", "ly", "rx", "ry", "accel",
    "brake", "start", "coin", "service", "test", "card",
];

/// The game's controls (`controls`, without those set to none in it or in `native_map`).
pub fn rows(profile: &Profile, device: Device) -> Vec<Row> {
    let hidden = |key: &str, label: &str| {
        label.eq_ignore_ascii_case("none")
            || profile.native_map.get(key).is_some_and(|n| n.eq_ignore_ascii_case("none"))
    };
    let mut out = Vec::new();
    for target in ORDER {
        let Some(label) = profile.controls.get(target) else { continue };
        if hidden(target, label) {
            continue;
        }
        let row = |kind, label: String| Row { target: target.to_string(), kind, label };
        match Axis::from_name(target) {
            None => out.push(row(RowKind::Button, label.clone())),
            Some(a) if a.is_pedal() => out.push(row(RowKind::Pedal, label.clone())),
            Some(a) => {
                if device != Device::Keyboard {
                    out.push(row(RowKind::Analog, label.clone()));
                }
                let (neg, pos) = if matches!(a, Axis::LeftX | Axis::RightX) { ("left", "right") } else { ("up", "down") };
                for (kind, sign, dir) in [(RowKind::Negative, '-', neg), (RowKind::Positive, '+', pos)] {
                    match profile.controls.get(&format!("{sign}{target}")) {
                        Some(l) if l.eq_ignore_ascii_case("none") => {}
                        Some(l) => out.push(row(kind, l.clone())),
                        None => out.push(row(kind, format!("{label} {dir}"))),
                    }
                }
            }
        }
    }
    out
}

/// A whole analog axis (`leftx`, `axis2`), not half of one or a trigger.
fn full_axis(device: Device, src: &str) -> bool {
    let s = src.to_ascii_lowercase();
    match device {
        Device::Keyboard => false,
        Device::Gamepad => ["leftx", "lefty", "rightx", "righty"].contains(&s.as_str()),
        Device::Wheel => s.starts_with("axis"),
    }
}

/// The row an entry belongs to: (target, kind), and whether it inverts its axis.
pub fn row_of(device: Device, src: &str, dst: &str) -> Option<(String, RowKind, bool)> {
    match mapping::parse_target(dst)? {
        Target::Button(_) => Some((dst.to_ascii_lowercase(), RowKind::Button, false)),
        Target::Axis { axis, invert } => {
            let target = axis.name().to_string();
            Some(if axis.is_pedal() {
                (target, RowKind::Pedal, invert)
            } else if full_axis(device, src) {
                (target, RowKind::Analog, invert)
            } else if invert {
                (target, RowKind::Negative, false)
            } else {
                (target, RowKind::Positive, false)
            })
        }
    }
}

/// The sources bound to a row: (source, inverted axis).
pub fn bound(table: &MapTable, device: Device, row: &Row) -> Vec<(String, bool)> {
    table
        .iter()
        .filter(|(_, d)| !d.eq_ignore_ascii_case("none"))
        .filter_map(|(s, d)| {
            let (t, k, inv) = row_of(device, s, d)?;
            (t == row.target && k == row.kind).then(|| (s.clone(), inv))
        })
        .collect()
}

/// A physical input caught while the GUI waits for one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Captured {
    Key(u16),
    PadButton(&'static str),
    /// Gamepad axis name and value (sticks centered on 0, triggers 0..32767).
    PadAxis(&'static str, i32),
    JoyButton(u8),
    /// Hat index, direction bit (1 up, 2 right, 4 down, 8 left).
    JoyHat(u8, u8),
    /// Axis index, value, value at rest (a pedal rests at one end).
    JoyAxis(u8, i32, i32),
}

/// A raw axis resting at one end: a pedal.
fn pedal_rest(rest: i32) -> bool {
    rest.abs() > 16000
}

/// The entry binding `c` to `row`: (source, target). None when it cannot drive it (a key for
/// a whole axis).
pub fn binding(c: Captured, row: &Row) -> Option<(String, String)> {
    let half = |sign: bool, name: &str| format!("{}{name}", if sign { '+' } else { '-' });
    let (src, invert) = match c {
        Captured::Key(code) => (format!("{:?}", evdev::KeyCode::new(code)), false),
        Captured::PadButton(name) => (name.to_string(), false),
        Captured::JoyButton(i) => (format!("button{i}"), false),
        Captured::JoyHat(i, bit) => {
            let dir = match bit {
                1 => "up",
                2 => "right",
                4 => "down",
                _ => "left",
            };
            (format!("hat{i}{dir}"), false)
        }
        Captured::PadAxis(name @ ("l2" | "r2"), _) => (name.to_string(), false),
        Captured::PadAxis(name, v) => match row.kind {
            // moving toward the negative side: inverted
            RowKind::Analog => (name.to_string(), v < 0),
            _ => (half(v > 0, name), false),
        },
        Captured::JoyAxis(i, v, rest) => {
            let name = format!("axis{i}");
            let moved_up = v > rest;
            match row.kind {
                RowKind::Analog => (name, !moved_up),
                // a pedal axis whole: from its rest end to the other
                RowKind::Pedal if pedal_rest(rest) => (name, rest > 0),
                _ => (half(moved_up, &name), false),
            }
        }
    };
    // a whole axis only from a stick or wheel axis (a trigger is half of one)
    let digital = matches!(
        c,
        Captured::Key(_) | Captured::PadButton(_) | Captured::JoyButton(_) | Captured::JoyHat(..) | Captured::PadAxis("l2" | "r2", _)
    );
    if digital && row.kind == RowKind::Analog {
        return None;
    }
    Some((src, row.dst(invert)))
}

/// Sets `src -> dst` in a table (`replace`: the row's other inputs removed), the same physical
/// input under another name (`lb` for `l1`) replaced.
pub fn assign(table: &mut MapTable, device: Device, row: &Row, src: &str, dst: &str, replace: bool) {
    if replace {
        for (s, _) in bound(table, device, row) {
            table.remove(&s);
        }
    }
    let parsed = device.parse(src);
    table.retain(|s, _| parsed.is_none() || device.parse(s) != parsed);
    table.insert(src.to_string(), dst.to_string());
}

/// The user layer of a table: the entries that differ from `base`, removed ones as none.
pub fn diff(base: &MapTable, edited: &MapTable) -> MapTable {
    let live = |t: &MapTable| -> MapTable {
        t.iter().filter(|(_, d)| !d.eq_ignore_ascii_case("none")).map(|(s, d)| (s.clone(), d.clone())).collect()
    };
    let (base, edited) = (live(base), live(edited));
    let mut out = MapTable::new();
    for (s, d) in &edited {
        if base.get(s) != Some(d) {
            out.insert(s.clone(), d.clone());
        }
    }
    for s in base.keys().filter(|s| !edited.contains_key(*s)) {
        out.insert(s.clone(), "none".into());
    }
    out
}

/// The table at `path` in a profile layer (empty when missing).
pub fn table_at(layer: &Value, path: &[&str]) -> MapTable {
    let mut v = layer;
    for k in path {
        match v.get(*k) {
            Some(next) => v = next,
            None => return MapTable::new(),
        }
    }
    serde_yaml_ng::from_value(v.clone()).unwrap_or_default()
}

/// Sets `path` in a profile layer (None: removes it, and the maps left empty).
pub fn set_at(layer: &mut Value, path: &[&str], value: Option<Value>) {
    fn go(v: &mut Value, path: &[&str], value: Option<Value>) {
        if !v.is_mapping() {
            *v = Value::Mapping(Mapping::new());
        }
        let map = v.as_mapping_mut().expect("mapping");
        let key = Value::String(path[0].to_string());
        if path.len() == 1 {
            match value {
                Some(x) => {
                    map.insert(key, x);
                }
                None => {
                    map.remove(&key);
                }
            }
            return;
        }
        if value.is_none() && !map.contains_key(&key) {
            return;
        }
        let child = map.entry(key.clone()).or_insert_with(|| Value::Mapping(Mapping::new()));
        go(child, &path[1..], value);
        if child.as_mapping().is_some_and(Mapping::is_empty) {
            map.remove(&key);
        }
    }
    go(layer, path, value)
}

/// A table as a YAML value (None when empty).
pub fn table_value(t: &MapTable) -> Option<Value> {
    (!t.is_empty()).then(|| serde_yaml_ng::to_value(t).expect("table"))
}

/// A physical input as its device names it.
pub fn source_label(device: Device, src: &str) -> String {
    match device {
        Device::Keyboard => key_label(src),
        Device::Gamepad => pad_label(src),
        Device::Wheel => joy_label(src),
    }
}

fn key_label(src: &str) -> String {
    let up = src.to_ascii_uppercase();
    let name = up.strip_prefix("KEY_").or_else(|| up.strip_prefix("BTN_")).unwrap_or(&up);
    let side = |rest: &str| match rest {
        "CTRL" => Some("Ctrl"),
        "SHIFT" => Some("Shift"),
        "ALT" => Some("Alt"),
        "META" => Some("Super"),
        "BRACE" => Some("Bracket"),
        _ => None,
    };
    if let Some(n) = name.strip_prefix("LEFT").and_then(side) {
        return format!("Left {n}");
    }
    if let Some(n) = name.strip_prefix("RIGHT").and_then(side) {
        return format!("Right {n}");
    }
    if let Some(k) = name.strip_prefix("KP") {
        let k = match k {
            "DOT" => ".",
            "ENTER" => "Enter",
            "PLUS" => "+",
            "MINUS" => "-",
            "ASTERISK" => "*",
            "SLASH" => "/",
            k => k,
        };
        return format!("Keypad {k}");
    }
    let named = match name {
        "UP" => "Up arrow",
        "DOWN" => "Down arrow",
        "LEFT" => "Left arrow",
        "RIGHT" => "Right arrow",
        "PAGEUP" => "Page Up",
        "PAGEDOWN" => "Page Down",
        "BACKSPACE" => "Backspace",
        "CAPSLOCK" => "Caps Lock",
        "ESC" => "Esc",
        "SPACE" => "Space",
        "ENTER" => "Enter",
        "TAB" => "Tab",
        "HOME" => "Home",
        "END" => "End",
        "INSERT" => "Insert",
        "DELETE" => "Delete",
        "GRAVE" => "`",
        "MINUS" => "-",
        "EQUAL" => "=",
        "SEMICOLON" => ";",
        "APOSTROPHE" => "'",
        "COMMA" => ",",
        "DOT" => ".",
        "SLASH" => "/",
        "BACKSLASH" => "\\",
        n => return n.to_string(),
    };
    named.to_string()
}

fn pad_label(src: &str) -> String {
    let lower = src.to_ascii_lowercase();
    let (sign, base) = match lower.chars().next() {
        Some(c @ ('+' | '-')) => (Some(c), &lower[1..]),
        _ => (None, lower.as_str()),
    };
    let stick = |name: &str, x: bool| -> String {
        match (sign, x) {
            (None, _) => format!("{name} {}", if x { "X" } else { "Y" }),
            (Some('+'), true) => format!("{name} right"),
            (Some(_), true) => format!("{name} left"),
            (Some('+'), false) => format!("{name} down"),
            (Some(_), false) => format!("{name} up"),
        }
    };
    match base {
        "a" | "south" => "A".into(),
        "b" | "east" => "B".into(),
        "x" | "west" => "X".into(),
        "y" | "north" => "Y".into(),
        "back" | "select" => "View (Back)".into(),
        "guide" | "home" => "Guide".into(),
        "start" => "Start (Menu)".into(),
        "l3" | "leftstick" => "Left stick click (L3)".into(),
        "r3" | "rightstick" => "Right stick click (R3)".into(),
        "l1" | "lb" | "leftshoulder" => "LB (L1)".into(),
        "r1" | "rb" | "rightshoulder" => "RB (R1)".into(),
        "l2" | "lt" | "lefttrigger" => "LT (L2)".into(),
        "r2" | "rt" | "righttrigger" => "RT (R2)".into(),
        "dpup" => "D-pad up".into(),
        "dpdown" => "D-pad down".into(),
        "dpleft" => "D-pad left".into(),
        "dpright" => "D-pad right".into(),
        "misc1" => "Share".into(),
        "paddle1" => "Paddle 1".into(),
        "paddle2" => "Paddle 2".into(),
        "paddle3" => "Paddle 3".into(),
        "paddle4" => "Paddle 4".into(),
        "touchpad" => "Touchpad".into(),
        "leftx" => stick("Left stick", true),
        "lefty" => stick("Left stick", false),
        "rightx" => stick("Right stick", true),
        "righty" => stick("Right stick", false),
        _ => src.to_string(),
    }
}

fn joy_label(src: &str) -> String {
    let lower = src.to_ascii_lowercase();
    let (sign, base) = match lower.chars().next() {
        Some(c @ ('+' | '-')) => (Some(c), &lower[1..]),
        _ => (None, lower.as_str()),
    };
    if let Some(i) = base.strip_prefix("button") {
        return format!("Button {i}");
    }
    if let Some(i) = base.strip_prefix("axis") {
        return match sign {
            Some(s) => format!("Axis {i} {s}"),
            None => format!("Axis {i}"),
        };
    }
    if let Some(h) = base.strip_prefix("hat") {
        let digits = h.chars().take_while(char::is_ascii_digit).count();
        return format!("Hat {} {}", &h[..digits], &h[digits..]);
    }
    src.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(target: &str, kind: RowKind) -> Row {
        Row { target: target.into(), kind, label: String::new() }
    }

    fn table(entries: &[(&str, &str)]) -> MapTable {
        entries.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn captures_to_entries() {
        let b = |c, r: &Row| binding(c, r).map(|(s, d)| format!("{s}:{d}"));
        assert_eq!(b(Captured::Key(29), &row("b1", RowKind::Button)).unwrap(), "KEY_LEFTCTRL:b1");
        assert_eq!(b(Captured::Key(105), &row("lx", RowKind::Negative)).unwrap(), "KEY_LEFT:-lx");
        assert_eq!(b(Captured::Key(105), &row("lx", RowKind::Analog)), None);
        assert_eq!(b(Captured::PadAxis("r2", 30000), &row("accel", RowKind::Pedal)).unwrap(), "r2:accel");
        assert_eq!(b(Captured::PadAxis("leftx", -30000), &row("lx", RowKind::Analog)).unwrap(), "leftx:-lx");
        assert_eq!(b(Captured::PadAxis("lefty", -30000), &row("b1", RowKind::Button)).unwrap(), "-lefty:b1");
        // wheel turned right, pedal resting released at +32767 then pressed
        assert_eq!(b(Captured::JoyAxis(0, 12000, 0), &row("lx", RowKind::Analog)).unwrap(), "axis0:lx");
        assert_eq!(b(Captured::JoyAxis(2, -20000, 32767), &row("accel", RowKind::Pedal)).unwrap(), "axis2:-accel");
        assert_eq!(b(Captured::JoyAxis(2, 20000, -32768), &row("brake", RowKind::Pedal)).unwrap(), "axis2:brake");
        assert_eq!(b(Captured::JoyHat(0, 8), &row("b2", RowKind::Button)).unwrap(), "hat0left:b2");
    }

    #[test]
    fn rows_of_entries() {
        let d = Device::Gamepad;
        assert_eq!(row_of(d, "leftx", "lx"), Some(("lx".into(), RowKind::Analog, false)));
        assert_eq!(row_of(d, "dpleft", "-lx"), Some(("lx".into(), RowKind::Negative, false)));
        assert_eq!(row_of(Device::Wheel, "axis2", "-accel"), Some(("accel".into(), RowKind::Pedal, true)));
        assert_eq!(row_of(d, "a", "B5"), Some(("b5".into(), RowKind::Button, false)));
        let t = table(&[("x", "b1"), ("r1", "b1"), ("a", "none"), ("leftx", "lx")]);
        assert_eq!(bound(&t, d, &row("b1", RowKind::Button)).len(), 2);
        assert!(bound(&t, d, &row("b5", RowKind::Button)).is_empty());
    }

    #[test]
    fn assign_and_diff() {
        let base = table(&[("x", "b1"), ("y", "b2"), ("lb", "b4")]);
        let mut t = base.clone();
        // l1 is lb: replaced; set (replace) on b1 drops x
        assign(&mut t, Device::Gamepad, &row("b1", RowKind::Button), "l1", "b1", true);
        assert_eq!(t, table(&[("y", "b2"), ("l1", "b1")]));
        assert_eq!(diff(&base, &t), table(&[("x", "none"), ("lb", "none"), ("l1", "b1")]));
        assert!(diff(&base, &base).is_empty());
    }

    #[test]
    fn layer_paths() {
        let mut v = Value::Mapping(Mapping::new());
        set_at(&mut v, &["input", "keyboard", "p1"], table_value(&table(&[("KEY_A", "b1")])));
        assert_eq!(table_at(&v, &["input", "keyboard", "p1"]), table(&[("KEY_A", "b1")]));
        set_at(&mut v, &["input", "keyboard", "p1"], None);
        assert!(v.as_mapping().unwrap().is_empty());
    }

    #[test]
    fn labels() {
        assert_eq!(source_label(Device::Keyboard, "KEY_LEFTCTRL"), "Left Ctrl");
        assert_eq!(source_label(Device::Keyboard, "KEY_KP8"), "Keypad 8");
        assert_eq!(source_label(Device::Keyboard, "KEY_Z"), "Z");
        assert_eq!(source_label(Device::Gamepad, "-leftx"), "Left stick left");
        assert_eq!(source_label(Device::Gamepad, "back"), "View (Back)");
        assert_eq!(source_label(Device::Wheel, "-axis1"), "Axis 1 -");
        assert_eq!(source_label(Device::Wheel, "hat0up"), "Hat 0 up");
    }
}
