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
    pub args: Vec<String>,
    pub runner: String,
    pub runners_dir: PathBuf,
    pub prefix: PathBuf,
    pub payloads_dir: PathBuf,
    pub wow64: bool,
    pub wine_debug: String,
    pub dxvk: bool,
    pub dxvk_from: String,
    pub graphics: Graphics,
    pub lib32_dirs: Vec<PathBuf>,
    pub lib64_dirs: Vec<PathBuf>,
    pub prefix_tricks: Vec<String>,
    pub tricks: Vec<String>,
    pub dll_overrides: BTreeMap<String, String>,
    pub hide: Vec<String>,
    pub reshade: bool,
    pub reshade_files: Vec<String>,
    pub exe_fixed_base: bool,
    pub gamescope: GamescopeConfig,
    pub files: BTreeMap<String, PathBuf>,
    pub initial_files: BTreeMap<String, PathBuf>,
    pub env: BTreeMap<String, String>,
    pub native_map: BTreeMap<String, String>,
    pub port: u16,
    pub input: InputConfig,

    /// Game executable, Windows path (`Z:\...`), from the dump's `.windowsloader` file.
    /// Empty when the profile was loaded by id (`show`, `input-test`).
    #[serde(skip)]
    pub exe: String,
    /// Folder levels between the dump root and the executable.
    #[serde(skip)]
    pub exe_depth: usize,
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
pub struct GamescopeConfig {
    pub enabled: bool,
    pub bin: PathBuf,
    pub width: u32,
    pub height: u32,
    pub args: Vec<String>,
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
    pub guns_enabled: bool,
    pub guns_mouse: bool,
    pub guns_mice: usize,
    pub mouse_screen: [u32; 2],
    pub gun: MapTable,
    pub devices: BTreeMap<String, DeviceConfig>,
    pub ffb: FfbConfig,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct FfbConfig {
    pub enabled: bool,
    pub gain: u32,
    pub invert: bool,
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

/// Extension of the file at the root of a game dump: `<gameid>.windowsloader`, whose first line
/// is the game executable's path relative to the dump root (`/` or `\\`).
pub const DUMP_EXT: &str = "windowsloader";

/// A game dump, found from its directory or its `.windowsloader` file.
#[derive(Debug)]
pub struct Dump {
    /// Game id: the system profile `systemprofiles/<system>/<id>.yaml`.
    pub id: String,
    /// Dump root (the `.windowsloader` file's directory), absolute.
    pub root: PathBuf,
    /// Executable path relative to the root.
    pub exe: PathBuf,
}

impl Dump {
    /// `arg` is a dump directory (with one `.windowsloader` file) or the file itself;
    /// `None` when it is neither (a profile id).
    pub fn find(arg: &str) -> Result<Option<Dump>> {
        let path = Path::new(arg);
        let file = if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)
                .with_context(|| format!("reading {arg}"))?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case(DUMP_EXT)))
                .collect();
            match found.len() {
                1 => found.remove(0),
                0 => bail!("{arg}: no <gameid>.{DUMP_EXT} file in this game dump"),
                _ => bail!("{arg}: several .{DUMP_EXT} files: {found:?}"),
            }
        } else if path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case(DUMP_EXT)) {
            path.to_path_buf()
        } else {
            return Ok(None);
        };
        let file = std::path::absolute(&file)?;
        let id = file.file_stem().context("dump file name")?.to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let rel = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .with_context(|| format!("{}: no executable path", file.display()))?;
        let exe: PathBuf = rel.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect();
        if exe.components().any(|c| !matches!(c, Component::Normal(_))) {
            bail!("{}: {rel:?} must be a path inside the dump", file.display());
        }
        let root = file.parent().context("dump root")?.to_path_buf();
        if !root.join(&exe).is_file() {
            bail!("{}: game executable not found: {}", file.display(), root.join(&exe).display());
        }
        Ok(Some(Dump { id, root, exe }))
    }
}

/// Where `systemprofiles/` is: `--root`, else the current directory, else next to the
/// launcher (its directory or the one above, e.g. `dist/..`).
fn profiles_root(root: Option<&Path>) -> Result<PathBuf> {
    if let Some(r) = root {
        return Ok(std::path::absolute(r)?);
    }
    let cwd = std::env::current_dir()?;
    let mut candidates = vec![cwd.clone()];
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().skip(1).take(2).map(Path::to_path_buf));
    }
    Ok(candidates.into_iter().find(|d| d.join(SYSTEM_PROFILES).is_dir()).unwrap_or(cwd))
}

impl Profile {
    /// Loads the profile of a game dump (its directory or `<gameid>.windowsloader` file): the
    /// game id selects `systemprofiles/<system>/<id>.yaml` (+ `userprofiles/`), the dump gives
    /// the executable. A game id (`<id>` or `<system>/<id>`) loads the profile alone.
    /// `extra`: more layers merged last (`--profile`, e.g. a frontend's per-game options).
    pub fn load(arg: &str, root: Option<&Path>, extra: &[PathBuf]) -> Result<Profile> {
        let root = profiles_root(root)?;
        let Some(dump) = Dump::find(arg)? else {
            return Profile::load_id(arg.trim_end_matches(".yaml"), &root, extra);
        };
        let mut profile = Profile::load_id(&dump.id, &root, extra)?;
        let exe = dump.root.join(&dump.exe);
        profile.exe = format!("Z:{}", exe.display()).replace('/', "\\");
        profile.exe_depth = dump.exe.components().count() - 1;
        Ok(profile)
    }

    /// The profile of game id `<id>` (searched in every system) or `<system>/<id>`.
    fn load_id(id: &str, root: &Path, extra: &[PathBuf]) -> Result<Profile> {
        let rel = if id.contains('/') {
            PathBuf::from(format!("{id}.yaml"))
        } else {
            let mut found: Vec<PathBuf> = std::fs::read_dir(root.join(SYSTEM_PROFILES))
                .with_context(|| format!("no {SYSTEM_PROFILES} in {}", root.display()))?
                .flatten()
                .map(|system| PathBuf::from(system.file_name()).join(format!("{id}.yaml")))
                .filter(|rel| root.join(SYSTEM_PROFILES).join(rel).is_file())
                .collect();
            match found.len() {
                1 => found.remove(0),
                0 => bail!("no system profile for game id {id:?} in {}/{SYSTEM_PROFILES}", root.display()),
                _ => bail!("game id {id:?} is ambiguous: {found:?}"),
            }
        };
        let root = root.to_path_buf();

        let mut value: Value = serde_yaml_ng::from_str(DEFAULTS).expect("valid defaults.yaml");
        let mut sources = Vec::new();
        let template = root.join(SYSTEM_PROFILES).join(&rel);
        let user = root.join(USER_PROFILES).join(&rel);
        if !template.exists() && !user.exists() {
            bail!("no profile {} in {}/{{{SYSTEM_PROFILES},{USER_PROFILES}}}", rel.display(), root.display());
        }
        for layer in [root.join("launcher.yaml"), template, user].into_iter().chain(extra.iter().cloned()) {
            if layer.exists() {
                merge(&mut value, read_yaml(&layer)?);
                sources.push(layer);
            }
        }
        if value.get("system").is_none_or(Value::is_null) {
            bail!("profile {}: 'system' is not set (in {sources:?})", rel.display());
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
        merge(&mut value, serde_yaml_ng::from_str("{system: none}")?);
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

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wal-profile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every system profile's `tricks` are preinstalled in the shared prefix (`prefix_tricks`),
    /// unless the profile has its own prefix.
    #[test]
    fn shared_prefix_has_every_trick() {
        let defaults: Value = serde_yaml_ng::from_str(DEFAULTS).unwrap();
        let shared: Vec<String> = serde_yaml_ng::from_value(defaults["prefix_tricks"].clone()).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(SYSTEM_PROFILES);
        let mut missing = Vec::new();
        for system in std::fs::read_dir(&root).unwrap().flatten().filter(|e| e.path().is_dir()) {
            for file in std::fs::read_dir(system.path()).unwrap().flatten() {
                if file.path().extension().is_none_or(|e| e != "yaml") {
                    continue;
                }
                let v = read_yaml(&file.path()).unwrap();
                if v.get("prefix").is_some() {
                    continue;
                }
                let tricks: Vec<String> = v.get("tricks").map(|t| serde_yaml_ng::from_value(t.clone()).unwrap()).unwrap_or_default();
                for t in tricks.iter().filter(|t| !shared.contains(t)) {
                    missing.push(format!("{}: {t}", file.path().display()));
                }
            }
        }
        assert!(missing.is_empty(), "tricks missing from defaults.yaml prefix_tricks: {missing:#?}");
    }

    #[test]
    fn dump_selects_profile_and_exe() {
        let dir = tmp("dump");
        std::fs::create_dir_all(dir.join("systemprofiles/nesica")).unwrap();
        std::fs::create_dir_all(dir.join("userprofiles/nesica")).unwrap();
        std::fs::write(
            dir.join("systemprofiles/nesica/game.yaml"),
            "system: nesica\nnative_map: {b5: btn6}\ninput: {gamepad: {a: b1}}\n",
        )
        .unwrap();
        std::fs::write(dir.join("userprofiles/nesica/game.yaml"), "input: {gamepad: {x: none}}\n").unwrap();
        let dump = dir.join("My Game");
        std::fs::create_dir_all(dump.join("bin")).unwrap();
        std::fs::write(dump.join("bin/Game.exe"), "").unwrap();
        std::fs::write(dump.join("game.windowsloader"), "# comment\nbin\\Game.exe\n").unwrap();

        let p = Profile::load(dump.to_str().unwrap(), Some(&dir), &[]).unwrap();
        assert_eq!(p.id, "nesica/game");
        assert_eq!(p.exe, format!("Z:{}", dump.join("bin/Game.exe").display()).replace('/', "\\"));
        assert_eq!(p.exe_depth, 1);
        assert_eq!(p.sources.len(), 2);
        assert_eq!(p.native_map["b5"], "btn6");
        assert_eq!(p.input.gamepad["a"], "b1");
        assert_eq!(p.input.gamepad["x"], "none");
        assert_eq!(p.input.gamepad["y"], "b2"); // default kept
        assert_eq!(p.graphics, Graphics::Wine);

        // the file itself, and the profile alone by id
        let p2 = Profile::load(dump.join("game.windowsloader").to_str().unwrap(), Some(&dir), &[]).unwrap();
        assert_eq!(p2.exe, p.exe);
        let p3 = Profile::load("game", Some(&dir), &[]).unwrap();
        // extra layers (--profile) are merged last
        std::fs::write(dir.join("extra.yaml"), "reshade: false\n").unwrap();
        let p4 = Profile::load("game", Some(&dir), &[dir.join("extra.yaml")]).unwrap();
        assert!(!p4.reshade && p3.reshade);
        assert_eq!((p3.id.as_str(), p3.exe.as_str()), ("nesica/game", ""));
        assert!(Profile::load("nesica/game", Some(&dir), &[]).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dump_errors() {
        let dir = tmp("errors");
        std::fs::create_dir_all(dir.join("systemprofiles/nesica")).unwrap();
        std::fs::create_dir_all(dir.join("systemprofiles/typex")).unwrap();
        std::fs::write(dir.join("systemprofiles/nesica/twin.yaml"), "system: nesica\n").unwrap();
        std::fs::write(dir.join("systemprofiles/typex/twin.yaml"), "system: typex\n").unwrap();
        let dump = dir.join("dump");
        std::fs::create_dir_all(&dump).unwrap();
        assert!(Profile::load(dump.to_str().unwrap(), Some(&dir), &[]).is_err()); // no file
        std::fs::write(dump.join("game.exe"), "").unwrap();
        std::fs::write(dump.join("twin.windowsloader"), "game.exe\n").unwrap();
        assert!(Profile::load(dump.to_str().unwrap(), Some(&dir), &[]).is_err()); // ambiguous id
        std::fs::write(dump.join("twin.windowsloader"), "../game.exe\n").unwrap();
        assert!(Dump::find(dump.to_str().unwrap()).is_err()); // outside the dump
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
