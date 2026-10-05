//! Game profiles: layered YAML (see `defaults.yaml` for the layers and every key).

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_yaml_ng::{Mapping, Value};

/// `source -> target` table: physical input name -> virtual stick input name.
pub type MapTable = BTreeMap<String, String>;

pub const SYSTEM_PROFILES: &str = "systemprofiles";
pub const USER_PROFILES: &str = "userprofiles";
const DEFAULTS: &str = include_str!("defaults.yaml");

/// Graphics stack for old DirectDraw/D3D games.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Graphics {
    Wine,
    D7vk,
    Dgvoodoo,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub system: String,
    pub name: Option<String>,
    pub exe: String,
    pub args: Vec<String>,
    pub exe_depth: usize,
    pub runner: String,
    pub runners_dir: PathBuf,
    pub prefix: PathBuf,
    pub payloads_dir: PathBuf,
    pub wow64: bool,
    pub wine_debug: String,
    pub dxvk: bool,
    pub graphics: Graphics,
    pub lib32_dirs: Vec<PathBuf>,
    pub lib64_dirs: Vec<PathBuf>,
    pub tricks: Vec<String>,
    pub hide: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub native_map: BTreeMap<String, String>,
    pub port: u16,
    pub input: InputConfig,

    /// Directory the relative paths are resolved from.
    #[serde(skip)]
    pub root: PathBuf,
    /// Profile id: `<system>/<game>`.
    #[serde(skip)]
    pub id: String,
    /// Files merged, in order.
    #[serde(skip)]
    pub sources: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    pub deadzone: i16,
    pub stick_as_dpad: bool,
    pub exit_combo: Vec<String>,
    pub gamepad: MapTable,
    pub keyboard_enabled: bool,
    pub keyboard: BTreeMap<String, MapTable>,
    pub devices: BTreeMap<String, DeviceConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub player: Option<usize>,
    #[serde(default)]
    pub raw: bool,
    #[serde(default)]
    pub map: MapTable,
}

/// Deep merge: maps key by key, anything else replaced.
fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Mapping(b), Value::Mapping(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}

fn read_yaml(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let v: Value = serde_yaml_ng::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(if v.is_null() { Value::Mapping(Mapping::new()) } else { v })
}

/// Splits `.../<root>/{systemprofiles,userprofiles}/<rel>` into (root, rel).
fn split_profile_path(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let comps: Vec<Component> = path.components().collect();
    let pos = comps.iter().rposition(|c| {
        matches!(c, Component::Normal(n) if *n == SYSTEM_PROFILES || *n == USER_PROFILES)
    })?;
    let root: PathBuf = comps[..pos].iter().collect();
    let rel: PathBuf = comps[pos + 1..].iter().collect();
    Some((if root.as_os_str().is_empty() { PathBuf::from(".") } else { root }, rel))
}

impl Profile {
    /// Loads a profile given as a file in `systemprofiles/` or `userprofiles/`, or as an
    /// id `<system>/<game>` resolved from `root`.
    pub fn load(arg: &str, root: Option<&Path>) -> Result<Profile> {
        let as_path = Path::new(arg);
        let (root, rel) = if as_path.is_file() {
            let abs = std::path::absolute(as_path)?;
            split_profile_path(&abs).with_context(|| {
                format!("{arg}: profiles must be in a '{SYSTEM_PROFILES}' or '{USER_PROFILES}' directory")
            })?
        } else {
            let root = root.map(Path::to_path_buf).unwrap_or(std::env::current_dir()?);
            let rel = PathBuf::from(if arg.ends_with(".yaml") { arg.to_string() } else { format!("{arg}.yaml") });
            (root, rel)
        };
        let root = std::path::absolute(root)?;

        let mut value: Value = serde_yaml_ng::from_str(DEFAULTS).expect("valid defaults.yaml");
        let mut sources = Vec::new();
        let template = root.join(SYSTEM_PROFILES).join(&rel);
        let user = root.join(USER_PROFILES).join(&rel);
        if !template.exists() && !user.exists() {
            bail!("no profile {} in {}/{{{SYSTEM_PROFILES},{USER_PROFILES}}}", rel.display(), root.display());
        }
        for layer in [root.join("launcher.yaml"), template, user] {
            if layer.exists() {
                merge(&mut value, read_yaml(&layer)?);
                sources.push(layer);
            }
        }
        for key in ["system", "exe"] {
            if value.get(key).is_none_or(Value::is_null) {
                bail!("profile {}: '{key}' is not set (in {sources:?})", rel.display());
            }
        }
        let mut profile: Profile = serde_yaml_ng::from_value(value)
            .with_context(|| format!("profile {} (from {sources:?})", rel.display()))?;
        profile.root = root;
        profile.id = rel.with_extension("").display().to_string();
        profile.sources = sources;
        Ok(profile)
    }

    /// Defaults and `launcher.yaml` only, without a game (input test).
    pub fn without_game(root: &Path) -> Result<Profile> {
        let root = std::path::absolute(root)?;
        let mut value: Value = serde_yaml_ng::from_str(DEFAULTS).expect("valid defaults.yaml");
        let mut sources = Vec::new();
        let global = root.join("launcher.yaml");
        if global.exists() {
            merge(&mut value, read_yaml(&global)?);
            sources.push(global);
        }
        merge(&mut value, serde_yaml_ng::from_str("{system: none, exe: ''}")?);
        let mut profile: Profile = serde_yaml_ng::from_value(value)?;
        profile.root = root;
        profile.sources = sources;
        Ok(profile)
    }

    pub fn path(&self, p: &Path) -> PathBuf {
        self.root.join(p)
    }

    /// Short name of the game, for directories.
    pub fn slug(&self) -> String {
        Path::new(&self.id).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_merge() {
        let dir = std::env::temp_dir().join(format!("wal-profile-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("systemprofiles/nesica")).unwrap();
        std::fs::create_dir_all(dir.join("userprofiles/nesica")).unwrap();
        std::fs::write(
            dir.join("systemprofiles/nesica/game.yaml"),
            "system: nesica\nexe: 'C:\\games\\g\\game.exe'\nnative_map: {b5: btn6}\ninput: {gamepad: {a: b1}}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("userprofiles/nesica/game.yaml"),
            "exe: 'Z:\\data\\g\\game.exe'\ninput: {gamepad: {x: none}}\n",
        )
        .unwrap();

        let p = Profile::load(dir.join("systemprofiles/nesica/game.yaml").to_str().unwrap(), None).unwrap();
        assert_eq!(p.exe, "Z:\\data\\g\\game.exe");
        assert_eq!(p.id, "nesica/game");
        assert_eq!(p.native_map["b5"], "btn6");
        assert_eq!(p.input.gamepad["a"], "b1");
        assert_eq!(p.input.gamepad["x"], "none");
        assert_eq!(p.input.gamepad["y"], "b2"); // default kept
        assert_eq!(p.graphics, Graphics::Wine);

        // by id, and from the user file
        let p2 = Profile::load("nesica/game", Some(&dir)).unwrap();
        assert_eq!(p2.sources.len(), 2);
        let p3 = Profile::load(dir.join("userprofiles/nesica/game.yaml").to_str().unwrap(), None).unwrap();
        assert_eq!(p3.exe, p.exe);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
