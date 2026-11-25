//! Gamepad button layout: which physical buttons form the 8-button arcade panel (top row
//! 1-4, bottom row 1-4), machine-wide (`userprofiles/pad-layout.yaml`, set in the GUI).
//!
//! The mapping tables (defaults, system and user profiles) are written for one reference
//! layout, `REFERENCE` (top X Y R1 L1, bottom A B R2 L2): they are never rewritten. When the
//! gamepad map is compiled, each of those 8 sources is moved to the physical button at the
//! same position in the chosen layout (`apply`): with the modern layout, a game's `l1` entry
//! (top row 4) is pressed with R2.
//!
//! The layout applies to every controller; a controller can have its own (`devices`, by a
//! case-insensitive substring of its SDL name).
//!
//! A game using one of the 8 as an axis (`r2: accel`, a trigger as a pedal) keeps its table
//! as written: a trigger stays its pedal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{MapTable, USER_PROFILES};
use crate::mapping::{self, Target};

const FILE: &str = "pad-layout.yaml";

/// The panel's 8 positions: top row left to right, then bottom row.
pub const POSITIONS: usize = 8;

/// The layout the mapping tables are written for.
pub const REFERENCE: [&str; POSITIONS] = ["x", "y", "r1", "l1", "a", "b", "r2", "l2"];
/// Modern arcade layout (the default): top X Y R1 R2, bottom A B L1 L2.
pub const MODERN: [&str; POSITIONS] = ["x", "y", "r1", "r2", "a", "b", "l1", "l2"];
/// Classic layout (Batocera's default MAME layout): top X Y L1 L2, bottom A B R1 R2.
pub const CLASSIC: [&str; POSITIONS] = ["x", "y", "l1", "l2", "a", "b", "r1", "r2"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Modern,
    Classic,
    Custom,
}

/// The two rows of a custom layout, left to right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rows {
    pub top: [String; 4],
    pub bottom: [String; 4],
}

impl Default for Rows {
    fn default() -> Rows {
        Rows::of(&MODERN)
    }
}

impl Rows {
    fn of(slots: &[&str; POSITIONS]) -> Rows {
        let s = |i: usize| slots[i].to_string();
        Rows { top: [s(0), s(1), s(2), s(3)], bottom: [s(4), s(5), s(6), s(7)] }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub layout: Kind,
    /// The custom layout (kept when another one is chosen).
    #[serde(default)]
    pub custom: Rows,
}

/// The machine's layouts: every controller's, and the controllers having their own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default)]
    pub all: Layout,
    /// Controller name (case-insensitive substring of the SDL name) -> its layout.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<String, Layout>,
}

impl Settings {
    /// The tables as written, for every controller.
    pub fn reference() -> Settings {
        let all = Layout { layout: Kind::Custom, custom: Rows::of(&REFERENCE) };
        Settings { all, devices: BTreeMap::new() }
    }

    /// The entry of `devices` matching a controller's name.
    pub fn device_entry(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.devices.keys().find(|m| !m.is_empty() && lower.contains(&m.to_ascii_lowercase())).map(String::as_str)
    }

    /// A controller's layout: its own, else every controller's.
    pub fn for_device(&self, name: &str) -> &Layout {
        self.device_entry(name).and_then(|m| self.devices.get(m)).unwrap_or(&self.all)
    }
}

impl Default for Layout {
    fn default() -> Layout {
        Layout { layout: Kind::Modern, custom: Rows::default() }
    }
}

/// One of the 8 panel buttons under any of its names (`lb` for `l1`).
pub fn canonical(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "a" | "south" => "a",
        "b" | "east" => "b",
        "x" | "west" => "x",
        "y" | "north" => "y",
        "l1" | "lb" | "leftshoulder" => "l1",
        "r1" | "rb" | "rightshoulder" => "r1",
        "l2" | "lt" | "lefttrigger" => "l2",
        "r2" | "rt" | "righttrigger" => "r2",
        _ => return None,
    })
}

/// A panel button's short label (`X`, `R1`).
pub fn label(name: &str) -> String {
    name.to_ascii_uppercase()
}

impl Layout {
    /// The physical buttons at the 8 positions.
    pub fn slots(&self) -> [&'static str; POSITIONS] {
        match self.layout {
            Kind::Modern => MODERN,
            Kind::Classic => CLASSIC,
            Kind::Custom => {
                let mut out = REFERENCE;
                for (i, n) in self.custom.top.iter().chain(&self.custom.bottom).enumerate() {
                    out[i] = canonical(n).unwrap_or(REFERENCE[i]);
                }
                out
            }
        }
    }

    /// Puts `name` at position `i` of a custom layout made from the current one, the button
    /// there taking `name`'s old position.
    pub fn assign(&mut self, i: usize, name: &str) {
        let Some(name) = canonical(name) else { return };
        let mut slots = self.slots();
        if let Some(j) = slots.iter().position(|s| *s == name) {
            slots[j] = slots[i];
        }
        slots[i] = name;
        self.custom = Rows::of(&slots);
        self.layout = Kind::Custom;
    }

    /// Whether the slots are the 8 buttons once each (a custom layout edited by hand may not
    /// be: then the reference layout is used).
    fn valid(&self) -> bool {
        let slots = self.slots();
        REFERENCE.iter().all(|b| slots.contains(b))
    }

    /// The physical button playing a table's source (`r2` for the reference `l1` in the
    /// modern layout); other names unchanged.
    pub fn to_physical(&self, src: &str) -> String {
        match canonical(src).and_then(|c| REFERENCE.iter().position(|r| *r == c)) {
            Some(i) if self.valid() => self.slots()[i].to_string(),
            _ => src.to_string(),
        }
    }

    /// The table's source a physical button plays (inverse of `to_physical`).
    pub fn to_reference(&self, src: &str) -> String {
        let slots = self.slots();
        match canonical(src).and_then(|c| slots.iter().position(|s| *s == c)) {
            Some(i) if self.valid() => REFERENCE[i].to_string(),
            _ => src.to_string(),
        }
    }

    /// Whether the layout moves `table`'s buttons: not for the reference layout itself, nor a
    /// table using one of the 8 buttons as an axis (a trigger as a pedal).
    pub fn applies(&self, table: &MapTable) -> bool {
        self.valid() && self.slots() != REFERENCE && !uses_axis(table)
    }

    /// `table` with the 8 panel sources moved to the layout's physical buttons.
    pub fn apply(&self, table: &MapTable) -> MapTable {
        if !self.applies(table) {
            return table.clone();
        }
        let mut out = MapTable::new();
        for (src, dst) in table {
            let key = self.to_physical(src);
            match out.get_mut(&key) {
                // the same button under two names (`l1` and `lb`): both targets
                Some(prev) if !dst.eq_ignore_ascii_case("none") => {
                    if prev.eq_ignore_ascii_case("none") {
                        *prev = dst.clone();
                    } else {
                        *prev = format!("{prev},{dst}");
                    }
                }
                Some(_) => {}
                None => {
                    out.insert(key, dst.clone());
                }
            }
        }
        out
    }
}

/// One of the 8 panel buttons drives an axis in `table`.
fn uses_axis(table: &MapTable) -> bool {
    table.iter().any(|(src, dst)| {
        canonical(src).is_some()
            && dst.split(',').any(|d| matches!(mapping::parse_target(d.trim()), Some(Target::Axis { .. })))
    })
}

fn path(root: &Path) -> PathBuf {
    root.join(USER_PROFILES).join(FILE)
}

/// The machine's layouts (modern for every controller when never set).
pub fn load(root: &Path) -> Result<Settings> {
    let path = path(root);
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let layout: Option<Settings> = serde_yaml_ng::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(layout.unwrap_or_default())
}

pub fn save(root: &Path, layout: &Settings) -> Result<()> {
    let path = path(root);
    std::fs::create_dir_all(root.join(USER_PROFILES))?;
    let text = serde_yaml_ng::to_string(layout)?;
    std::fs::write(&path, format!("# Gamepad button layouts (launcher GUI): modern, classic or custom; all: every controller, devices: by name\n{text}"))
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(entries: &[(&str, &str)]) -> MapTable {
        entries.iter().map(|(s, d)| (s.to_string(), d.to_string())).collect()
    }

    const DEFAULT: [(&str, &str); 9] = [
        ("x", "b1"),
        ("y", "b2"),
        ("r1", "b3"),
        ("l1", "b4"),
        ("a", "b5"),
        ("b", "b6"),
        ("r2", "b7"),
        ("l2", "b8"),
        ("start", "start"),
    ];

    #[test]
    fn modern_moves_top_4_and_bottom_3() {
        let out = Layout::default().apply(&t(&DEFAULT));
        assert_eq!(out["r2"], "b4");
        assert_eq!(out["l1"], "b7");
        assert_eq!(out["r1"], "b3");
        assert_eq!(out["l2"], "b8");
        assert_eq!(out["start"], "start");
    }

    #[test]
    fn classic_rows() {
        let l = Layout { layout: Kind::Classic, ..Default::default() };
        let out = l.apply(&t(&DEFAULT));
        assert_eq!(t(&[("x", "b1"), ("y", "b2"), ("l1", "b3"), ("l2", "b4"), ("a", "b5"), ("b", "b6"), ("r1", "b7"), ("r2", "b8"), ("start", "start")]), out);
        for b in REFERENCE {
            assert_eq!(l.to_reference(&l.to_physical(b)), b);
        }
    }

    #[test]
    fn triggers_as_pedals_keep_the_table() {
        let table = t(&[("x", "b1"), ("r1", "b2"), ("r2", "accel"), ("l2", "brake")]);
        assert_eq!(Layout::default().apply(&table), table);
    }

    #[test]
    fn custom_and_invalid_custom() {
        let mut l = Layout { layout: Kind::Custom, custom: Rows::of(&REFERENCE) };
        assert!(!l.applies(&t(&DEFAULT)));
        l.custom.top[0] = "a".into(); // a twice, x missing: ignored
        assert_eq!(l.apply(&t(&DEFAULT)), t(&DEFAULT));
        l.custom.bottom[0] = "x".into(); // swapped
        let out = l.apply(&t(&DEFAULT));
        assert_eq!((out["a"].as_str(), out["x"].as_str()), ("b1", "b5"));
    }

    #[test]
    fn assign_swaps() {
        let mut l = Layout::default();
        l.assign(3, "l1"); // modern top 4 = r2, l1 at bottom 3
        assert_eq!(l.layout, Kind::Custom);
        assert_eq!(l.slots(), ["x", "y", "r1", "l1", "a", "b", "r2", "l2"]);
    }

    #[test]
    fn device_layouts() {
        let text = "all: {layout: classic}\ndevices:\n  arcade stick: {layout: modern}\n";
        let s: Settings = serde_yaml_ng::from_str(text).unwrap();
        assert_eq!(s.for_device("8BitDo Arcade Stick").layout, Kind::Modern);
        assert_eq!(s.for_device("Xbox Controller").layout, Kind::Classic);
        assert_eq!(serde_yaml_ng::from_str::<Settings>("{}").unwrap().all, Layout::default());
    }

    #[test]
    fn alias_names() {
        let out = Layout::default().apply(&t(&[("lb", "b4"), ("x", "b1")]));
        assert_eq!(out["r2"], "b4");
    }
}
