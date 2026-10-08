//! Wine runner: environment, prefix setup and process launch.
//!
//! Follows batocera-wine: same library order (32-bit system libs, runner i386-unix,
//! 64-bit system libs, runner x86_64-unix, then the runner's own FFmpeg last), GE-Proton
//! prefixes get vkd3d and icu linked in, prefixes are updated when the runner changes.
//! A new prefix is unpacked from `<prefix>.tar.gz` when present (`tools/prefix-archive.sh`).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};

use crate::config::{Graphics, Profile};

pub struct Wine {
    pub runner: PathBuf,
    /// Runner whose DXVK is used when `runner` ships none (profile `dxvk_from`).
    dxvk_runner: Option<PathBuf>,
    pub prefix: PathBuf,
    /// Compute everything but change nothing on disk.
    pub dry_run: bool,
    env: Vec<(String, String)>,
    overrides: Vec<String>,
}

const D3D_DLLS: [&str; 5] = ["d3d8", "d3d9", "d3d10core", "d3d11", "dxgi"];

/// Verbs whose native DLLs break other games: native only for the games listing them.
const ISOLATED_TRICKS: [&str; 1] = ["dsound"];

/// DLLs a winetricks verb sets to native (its `w_override_dlls`).
fn trick_dlls(verb: &str) -> Vec<String> {
    match verb {
        "dmusic" => vec!["dmusic".into(), "dmusic32".into()],
        "xact" => {
            let mut dlls: Vec<String> = (0..8).map(|i| format!("xaudio2_{i}")).collect();
            dlls.extend((0..8).map(|i| format!("x3daudio1_{i}")));
            dlls.extend((1..6).map(|i| format!("xapofx1_{i}")));
            dlls.extend((0..11).map(|i| format!("xactengine2_{i}")));
            dlls.extend((0..8).map(|i| format!("xactengine3_{i}")));
            dlls
        }
        // registrations, sound bank, fonts: no DLL override
        "dsdmo" | "gmdls" | "fakejapanese" => vec![],
        _ => vec![verb.into()],
    }
}

fn existing_unique(candidates: &[&str]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for c in candidates {
        let p = Path::new(c);
        if let Ok(real) = p.canonicalize()
            && !out.iter().any(|o| o.canonicalize().ok().as_ref() == Some(&real))
        {
            out.push(p.to_path_buf());
        }
    }
    out
}

/// `<root>/<lib dir>/<component>/<arch>`, across runner layouts.
fn component_in(root: &Path, component: &str, arch: &str) -> Option<PathBuf> {
    ["lib/wine", "lib", "lib64/wine", "lib32/wine"]
        .iter()
        .map(|l| root.join(l).join(component).join(arch))
        .find(|p| p.is_dir())
}

fn join_paths(paths: &[PathBuf]) -> String {
    paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(":")
}

impl Wine {
    pub fn new(config: &Profile, dry_run: bool) -> Result<Wine> {
        let runner = config.path(&config.runners_dir).join(&config.runner);
        if !runner.join("bin/wine").exists() {
            bail!("wine runner not found: {}", runner.display());
        }
        let prefix = config.path(&config.prefix);
        let dxvk_runner = (!config.dxvk_from.is_empty()).then(|| config.path(&config.runners_dir).join(&config.dxvk_from));
        let mut wine = Wine { runner, dxvk_runner, prefix, dry_run, env: Vec::new(), overrides: Vec::new() };

        let lib32 = if config.lib32_dirs.is_empty() {
            existing_unique(&["/lib32", "/usr/lib32"])
        } else {
            config.lib32_dirs.clone()
        };
        let lib64 = if config.lib64_dirs.is_empty() {
            existing_unique(&["/lib", "/usr/lib", "/lib64", "/usr/lib64"])
        } else {
            config.lib64_dirs.clone()
        };
        let r = &wine.runner;
        let mut ld: Vec<PathBuf> = lib32;
        ld.push(wine.builtin_dir("i386-unix"));
        ld.extend(lib64);
        ld.push(wine.builtin_dir("x86_64-unix"));
        // FFmpeg of the runner for winedmo, last so the system libraries keep priority
        for extra in ["lib/x86_64-linux-gnu", "lib/i386-linux-gnu"] {
            if r.join(extra).is_dir() {
                ld.push(r.join(extra));
            }
        }
        let mut ld = join_paths(&ld);
        if let Ok(user) = std::env::var("LD_LIBRARY_PATH")
            && !user.is_empty()
        {
            ld = format!("{ld}:{user}");
        }

        let ntsync = fs::File::options().read(true).write(true).open("/dev/ntsync").is_ok();
        let path = format!("{}:{}", r.join("bin").display(), std::env::var("PATH").unwrap_or_default());
        let winedllpath = join_paths(&[wine.builtin_dir("i386-windows"), wine.builtin_dir("x86_64-windows")]);
        wine.env = vec![
            ("WINEPREFIX".into(), wine.prefix.display().to_string()),
            // WINEDEBUG set in the shell wins, for debugging a game
            ("WINEDEBUG".into(), std::env::var("WINEDEBUG").unwrap_or_else(|_| config.wine_debug.clone())),
            ("WINEDLLPATH".into(), winedllpath),
            ("LD_LIBRARY_PATH".into(), ld),
            ("PATH".into(), path),
            ("WINEDISABLEFASTSYNC".into(), if ntsync { "0" } else { "1" }.into()),
            ("DXVK_LOG_LEVEL".into(), "none".into()),
            ("VKD3D_DEBUG".into(), "none".into()),
            ("WINE_MONO_OVERRIDES".into(), "Microsoft.Xna.Framework.*,Gac=n".into()),
        ];
        if config.wow64 {
            wine.set_env("WINEARCH", "wow64");
        }
        for (k, v) in &config.env {
            wine.set_env(k, v);
        }
        wine.overrides.push("winemenubuilder.exe=".into());
        Ok(wine)
    }

    pub fn set_env(&mut self, key: &str, value: &str) {
        self.env.retain(|(k, _)| k != key);
        self.env.push((key.into(), value.into()));
    }

    /// `dll[,dll...]=mode` in WINEDLLOVERRIDES. Wine looks up `*<dll>` first, in the
    /// environment then in the registry, before the plain name: winetricks writes `*dsound`
    /// keys, which would win over a plain `dsound=b`. The names get the `*` too.
    pub fn override_dll(&mut self, spec: &str) {
        let (dlls, mode) = spec.split_once('=').unwrap_or((spec, ""));
        let dlls: Vec<String> = dlls.split(',').map(|d| format!("*{}", d.trim_start_matches('*'))).collect();
        self.overrides.push(format!("{}={mode}", dlls.join(",")));
    }

    /// The shared prefix holds every game's verbs and their native DLLs serve every game (as the
    /// old common prefix did: games rely on d3dx9, xact... they do not list), except the
    /// `ISOLATED_TRICKS` ones, native only for the games listing them (native dsound crashes
    /// the CRI audio games). Its `dll_overrides` still have the last word.
    pub fn builtin_unlisted_tricks(&mut self, prefix_tricks: &[String], tricks: &[String], keep: &[&str]) {
        let needed: Vec<String> = tricks.iter().flat_map(|v| trick_dlls(v)).collect();
        let mut builtin: Vec<String> = prefix_tricks
            .iter()
            .filter(|v| ISOLATED_TRICKS.contains(&v.as_str()))
            .flat_map(|v| trick_dlls(v))
            .filter(|d| !needed.contains(d) && !keep.contains(&d.as_str()))
            .collect();
        builtin.sort();
        builtin.dedup();
        if !builtin.is_empty() {
            self.override_dll(&format!("{}=b", builtin.join(",")));
        }
    }

    pub fn env(&self) -> Vec<(String, String)> {
        let mut env = self.env.clone();
        env.push(("WINEDLLOVERRIDES".into(), self.overrides.join(";")));
        env
    }

    /// Wine builtin dll directory (`i386-windows`, `x86_64-unix`...), across runner layouts.
    fn builtin_dir(&self, arch: &str) -> PathBuf {
        ["lib/wine", "lib64/wine", "lib32/wine"]
            .iter()
            .map(|l| self.runner.join(l).join(arch))
            .find(|p| p.is_dir())
            .unwrap_or_else(|| self.runner.join("lib/wine").join(arch))
    }

    /// Optional component shipped by the runner (dxvk, d7vk, vkd3d, icu...).
    fn component_dir(&self, component: &str, arch: &str) -> Option<PathBuf> {
        component_in(&self.runner, component, arch)
    }

    pub fn system32(&self) -> PathBuf {
        self.prefix.join("drive_c/windows/system32")
    }

    pub fn syswow64(&self) -> PathBuf {
        self.prefix.join("drive_c/windows/syswow64")
    }

    /// `wine <program> <args>`, with the runner environment.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut cmd = Command::new(self.runner.join("bin/wine"));
        cmd.arg(program).envs(self.env());
        cmd
    }

    pub fn wineserver(&self, arg: &str) -> Result<ExitStatus> {
        Ok(Command::new(self.runner.join("bin/wineserver")).arg(arg).envs(self.env()).status()?)
    }

    fn wineboot(&self) -> Result<()> {
        let status = self
            .command("wineboot")
            .arg("-u")
            // no mono/gecko install dialogs
            .env("WINEDLLOVERRIDES", "winemenubuilder.exe=;mscoree=;mshtml=")
            .status()
            .context("running wineboot")?;
        self.wineserver("-w")?;
        if !status.success() {
            bail!("wineboot failed: {status}");
        }
        Ok(())
    }

    /// Creates the prefix (unpacked from `<prefix>.tar.gz` when present), or updates it when the
    /// runner changed.
    pub fn prepare_prefix(&self) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        self.unpack_prefix()?;
        let inf = self.runner.join("share/wine/wine.inf");
        let inf_stamp = fs::metadata(&inf)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();
        let stamp_file = self.prefix.join(".wal-update-timestamp");

        if !self.prefix.join("system.reg").exists() {
            eprintln!("wine: creating prefix {}", self.prefix.display());
            fs::create_dir_all(&self.prefix)?;
            self.wineboot()?;
        } else if fs::read_to_string(&stamp_file).unwrap_or_default().trim() != inf_stamp {
            eprintln!("wine: runner changed, updating prefix {}", self.prefix.display());
            // links are all recreated below
            for dir in [self.system32(), self.syswow64()] {
                for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_symlink()) {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
            self.wineboot()?;
        }
        fs::write(&stamp_file, &inf_stamp)?;

        // GE-Proton does not link vkd3d and icu in its builtin dlls
        for component in ["vkd3d", "icu"] {
            for (arch, dir) in [("x86_64-windows", self.system32()), ("i386-windows", self.syswow64())] {
                if let Some(src) = self.component_dir(component, arch) {
                    link_all(&src, &dir, |_| true)?;
                }
            }
        }
        Ok(())
    }

    /// A prefix built elsewhere (`<prefix>.tar.gz`, winetricks verbs installed), unpacked when
    /// the prefix does not exist yet. Without its runner stamp, the runner update below rewrites
    /// what points into the runner of the machine that built it (font paths, links); verbs
    /// added since are installed by `apply_tricks` (`.wal-tricks` of the archive).
    fn unpack_prefix(&self) -> Result<()> {
        let archive = self.prefix.with_file_name(format!(
            "{}.tar.gz",
            self.prefix.file_name().unwrap_or_default().to_string_lossy()
        ));
        if self.prefix.exists() || !archive.is_file() {
            return Ok(());
        }
        eprintln!("wine: unpacking prefix {}", archive.display());
        // unpacked aside then renamed: an interrupted unpack leaves no partial prefix
        let tmp = self.prefix.with_file_name(format!(
            "{}.unpack",
            self.prefix.file_name().unwrap_or_default().to_string_lossy()
        ));
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&tmp)
            .status()
            .context("running tar")?;
        if !status.success() {
            let _ = fs::remove_dir_all(&tmp);
            bail!("unpacking {} failed: {status}", archive.display());
        }
        let _ = fs::remove_file(tmp.join(".wal-update-timestamp"));
        fs::rename(&tmp, &self.prefix)?;
        Ok(())
    }

    /// Applies the winetricks verbs not yet applied to the prefix.
    pub fn apply_tricks(&self, tricks: &[String]) -> Result<()> {
        let stamp = self.prefix.join(".wal-tricks");
        let done = fs::read_to_string(&stamp).unwrap_or_default();
        for verb in tricks {
            if done.lines().any(|l| l.trim() == verb) {
                continue;
            }
            eprintln!("wine: applying winetricks {verb}");
            if self.dry_run {
                continue;
            }
            // the runner's own copy (bin/winetricks), the runners directory's (Batocera's
            // /userdata/system/wine/custom/winetricks), else the system's
            let winetricks = [self.runner.join("bin/winetricks"), self.runner.with_file_name("winetricks")]
                .into_iter()
                .find(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("winetricks"));
            let status = Command::new(&winetricks)
                .args(["-q", verb])
                .envs(self.env())
                // winetricks only knows win32/win64 prefixes
                .env_remove("WINEARCH")
                .env("WINE", self.runner.join("bin/wine"))
                .env("WINESERVER", self.runner.join("bin/wineserver"))
                .status()
                .with_context(|| format!("running {} (is it installed?)", winetricks.display()))?;
            self.wineserver("-w")?;
            if !status.success() {
                bail!("winetricks {verb} failed: {status}");
            }
            let mut list = fs::read_to_string(&stamp).unwrap_or_default();
            list.push_str(verb);
            list.push('\n');
            fs::write(&stamp, list)?;
        }
        Ok(())
    }

    /// d3d8..11/dxgi: the runner's DXVK (or `dxvk_from`'s), or wine's builtin wined3d.
    /// `game_first`: DLLs the game directory provides (ReShade...), loaded before wine's
    /// builtin ones in wined3d mode (DXVK is native already: the game directory comes first).
    pub fn setup_d3d(&mut self, dxvk: bool, game_first: &[String]) -> Result<()> {
        let dry = self.dry_run;
        let dxvk_dir_of = |arch| {
            self.component_dir("dxvk", arch).or_else(|| self.dxvk_runner.as_ref().and_then(|r| component_in(r, "dxvk", arch)))
        };
        let dxvk_dirs = (dxvk_dir_of("x86_64-windows"), dxvk_dir_of("i386-windows"));
        let use_dxvk = dxvk && dxvk_dirs.0.is_some();
        for (arch, dir, dxvk_dir) in
            [("x86_64-windows", self.system32(), dxvk_dirs.0), ("i386-windows", self.syswow64(), dxvk_dirs.1)]
        {
            let src = if use_dxvk { dxvk_dir.unwrap_or_else(|| self.builtin_dir(arch)) } else { self.builtin_dir(arch) };
            if !dry {
                link_all(&src, &dir, |name| D3D_DLLS.iter().any(|d| name.eq_ignore_ascii_case(&format!("{d}.dll"))))?;
            }
        }
        if use_dxvk {
            self.override_dll(&format!("{}=n", D3D_DLLS.join(",")));
        } else {
            let first = |d: &&str| game_first.iter().any(|f| f.eq_ignore_ascii_case(&format!("{d}.dll")));
            let (native, builtin): (Vec<&str>, Vec<&str>) = D3D_DLLS.iter().copied().partition(|d| first(&d));
            if !native.is_empty() {
                self.override_dll(&format!("{}=n,b", native.join(",")));
            }
            self.override_dll(&format!("{}=b", builtin.join(",")));
        }
        Ok(())
    }

    /// DirectDraw backend.
    pub fn setup_ddraw(&mut self, graphics: Graphics) -> Result<()> {
        let wow = self.syswow64();
        let builtin = self.builtin_dir("i386-windows").join("ddraw.dll");
        let link = |src: &Path, dst: &Path| if self.dry_run { Ok(()) } else { link(src, dst) };
        if !self.dry_run {
            let _ = fs::remove_file(wow.join("ddraw_.dll"));
        }
        match graphics {
            Graphics::D7vk => {
                let d7vk = self.component_dir("d7vk", "i386-windows").context("this runner has no d7vk")?;
                link(&d7vk.join("ddraw.dll"), &wow.join("ddraw.dll"))?;
                // d7vk hands what it does not implement to wine's ddraw
                link(&builtin, &wow.join("ddraw_.dll"))?;
                self.override_dll("ddraw=n,b");
            }
            Graphics::Wine => {
                link(&builtin, &wow.join("ddraw.dll"))?;
                self.override_dll("ddraw,d3dimm=b");
            }
            Graphics::Dgvoodoo => {
                link(&builtin, &wow.join("ddraw.dll"))?;
                // the game directory's dgVoodoo DDraw.dll/D3DImm.dll
                self.override_dll("ddraw,d3dimm=n,b");
            }
        }
        Ok(())
    }

    /// Unix path of a Windows path (`C:\...`, `Z:\...`, or any drive of the prefix
    /// `dosdevices`). Components are matched case insensitively, like Windows does.
    pub fn unix_path(&self, win: &str) -> Result<PathBuf> {
        let b = win.as_bytes();
        if b.len() < 2 || b[1] != b':' || !b[0].is_ascii_alphabetic() {
            bail!("'{win}' is not an absolute Windows path (C:\\... or Z:\\...)");
        }
        let letter = (b[0] as char).to_ascii_lowercase();
        let mut path = match self.prefix.join("dosdevices").join(format!("{letter}:")).canonicalize() {
            Ok(p) => p,
            Err(_) if letter == 'c' => self.prefix.join("drive_c"),
            Err(_) if letter == 'z' => PathBuf::from("/"),
            Err(_) => bail!("drive {letter}: is not configured in {}/dosdevices", self.prefix.display()),
        };
        for part in win[2..].split(['\\', '/']).filter(|p| !p.is_empty()) {
            let exact = path.join(part);
            path = if exact.exists() {
                exact
            } else {
                fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))
                    .map(|e| e.path())
                    .unwrap_or(exact)
            };
        }
        Ok(path)
    }

    /// Windows path of a unix path, for the game process.
    pub fn windows_path(&self, path: &Path) -> String {
        let drive_c = self.prefix.join("drive_c");
        let (drive, rest) = match path.strip_prefix(&drive_c) {
            Ok(rel) => ("C:\\", rel.to_path_buf()),
            Err(_) => ("Z:\\", path.strip_prefix("/").unwrap_or(path).to_path_buf()),
        };
        format!("{drive}{}", rest.display().to_string().replace('/', "\\"))
    }
}

fn link(src: &Path, dst: &Path) -> Result<()> {
    if fs::symlink_metadata(dst).is_ok() {
        fs::remove_file(dst).with_context(|| format!("removing {}", dst.display()))?;
    }
    std::os::unix::fs::symlink(src, dst).with_context(|| format!("linking {}", dst.display()))
}

fn link_all(src_dir: &Path, dst_dir: &Path, filter: impl Fn(&str) -> bool) -> Result<()> {
    fs::create_dir_all(dst_dir)?;
    for entry in fs::read_dir(src_dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.to_ascii_lowercase().ends_with(".dll") && filter(&name) {
            link(&entry.path(), &dst_dir.join(&name))?;
        }
    }
    Ok(())
}
