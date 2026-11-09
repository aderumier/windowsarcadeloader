//! Windows arcade game launcher for Linux.
//!
//! Reads physical inputs (SDL3 gamepads/wheels, evdev keyboards), maps them to virtual
//! arcade sticks, serves them over TCP to the payload DLL running inside the game, and
//! runs the game with a wine runner.

mod config;
mod ffb;
mod focus;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod glibc_compat;
mod guns;
mod gui;
mod input;
mod mapping;
mod eamuse_card;
mod nesica_card;
mod players;
mod rundir;
mod script;
mod server;
mod sinden;
mod squashfs;
mod systems;
mod touch;
mod wheels;
mod wine;

use std::process::Command;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use config::{Graphics, Profile};
use wal_protocol::{InputFrame, env};

const USAGE: &str = "\
usage:
  arcade-launcher [gui] [--root DIR]
      Without a game: the frontend window (every game, its controls mapped per device into
      its user profile, its dump folder, launch).
  arcade-launcher [run] <dump> [--root DIR] [--profile FILE]... [--dry-run] [--input-script FILE]
                  [--no-input] [--headless]
      Launch a game. <dump> is a game dump directory holding a <gameid>.windowsloader
      file (the executable path relative to the dump root), that file, or the dump packed
      as a SquashFS image (<name>.squashfs, mounted with an overlay for its writes in
      squashfs_saves/<image folder>/<name>: squashfuse and fuse-overlayfs, or root). The game id
      selects systemprofiles/<system>/<gameid>.yaml, merged with userprofiles/.
      --root: where systemprofiles/ is (default: the current directory, else next to
      the launcher). --profile merges a YAML layer last (frontend options, e.g. reshade:
      false), repeatable. --input-script replays timed virtual stick inputs
      (`<seconds> p<N> <inputs...>` per line, `-` releases), for tests. --no-input: no
      physical device drives the game (keyboards, pads, guns, exit keys), only the script.
      --headless: the game in gamescope's headless backend, no window on the desktop (its
      gamescope screen size, else 1920x1080). Both for background test runs.
  arcade-launcher input-test [<dump> | <gameid>] [--root DIR]
      Print the virtual arcade sticks while you press buttons.
  arcade-launcher show <dump> | <gameid> [--root DIR]
      Print the merged profile.
  arcade-launcher prepare <dump> | <gameid> [--root DIR] [--profile FILE]...
      Create the game's wine prefix and install its winetricks verbs (prefix_tricks and
      tricks), without running the game.";

struct Args {
    command: String,
    profile: Option<String>,
    root: Option<PathBuf>,
    layers: Vec<PathBuf>,
    dry_run: bool,
    script: Option<PathBuf>,
    no_input: bool,
    headless: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args { command: "run".into(), profile: None, root: None, layers: Vec::new(), dry_run: false, script: None, no_input: false, headless: false };
    let mut positional = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => args.root = Some(it.next().context("--root needs a directory")?.into()),
            "--profile" => args.layers.push(it.next().context("--profile needs a file")?.into()),
            "--dry-run" => args.dry_run = true,
            "--no-input" => args.no_input = true,
            "--headless" => args.headless = true,
            "--input-script" => args.script = Some(it.next().context("--input-script needs a file")?.into()),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ if a.starts_with("--") => bail!("unknown option {a}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    let mut positional = positional.into_iter();
    match positional.next() {
        Some(c) if ["run", "input-test", "show", "prepare", "gui"].contains(&c.as_str()) => {
            args.command = c;
            args.profile = positional.next();
        }
        Some(other) => args.profile = Some(other),
        None => args.command = "gui".into(),
    }
    Ok(args)
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args = parse_args()?;
    let dump = || args.profile.as_deref().with_context(|| format!("missing <dump>\n{USAGE}"));
    // a SquashFS image: its game's profile (read from the image, not mounted)
    let load = || -> Result<Profile> {
        let p = dump()?;
        let p = if squashfs::is_image(Path::new(p)) { squashfs::game_id(Path::new(p))? } else { p.to_string() };
        Profile::load(&p, args.root.as_deref(), &args.layers)
    };
    match args.command.as_str() {
        "gui" => gui::run(&config::profiles_root(args.root.as_deref())?),
        "show" => {
            let p = load()?;
            println!("# {} from {:?}\n{p:#?}", p.id, p.sources);
            Ok(())
        }
        "prepare" => {
            // the game's prefix, ready (created, its winetricks verbs installed)
            let p = load()?;
            let wine = wine::Wine::new(&p, false)?;
            wine.prepare_prefix()?;
            wine.apply_tricks(&p.prefix_tricks)?;
            wine.apply_tricks(&p.tricks)?;
            // registry written to disk (tools/prefix-archive.sh packs it next)
            wine.wineserver("-w")?;
            eprintln!("prefix ready: {}", wine.prefix.display());
            Ok(())
        }
        "input-test" => {
            let profile = match &args.profile {
                Some(_) => load()?,
                None => Profile::without_game(args.root.as_deref().unwrap_or(Path::new(".")))?,
            };
            input_test(&profile)
        }
        _ => {
            let script = args.script.as_deref().map(script::Script::load).transpose()?;
            let opts = RunOptions { dry_run: args.dry_run, no_input: args.no_input, headless: args.headless };
            let image = Path::new(dump()?);
            if !squashfs::is_image(image) {
                return run(&load()?, &opts, script);
            }
            // mounted with its overlay for the time of the game
            let saves = load()?.path(&load()?.squashfs_saves);
            let mount = squashfs::Mount::new(image, &saves)?;
            let profile = Profile::load(&mount.dir().to_string_lossy(), args.root.as_deref(), &args.layers)?;
            run(&profile, &opts, script)
        }
    }
}

/// The process' main thread has exited (zombie state) while it is not reaped yet.
fn main_thread_dead(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(')').map(|(_, rest)| rest.trim_start().starts_with('Z')))
        .unwrap_or(false)
}

fn stop_flag() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    ctrlc::set_handler(move || s.store(true, Ordering::SeqCst))?;
    Ok(stop)
}

fn describe(frame: &InputFrame) -> String {
    let mut out = String::new();
    for (i, p) in frame.players.iter().enumerate() {
        if p.buttons == 0 && p.axes.iter().all(|a| *a == 0) {
            continue;
        }
        let pressed: Vec<&str> =
            wal_protocol::button::NAMES.iter().filter(|(_, b)| p.pressed(*b)).map(|(n, _)| *n).collect();
        let axes: Vec<String> = wal_protocol::Axis::ALL
            .iter()
            .filter(|a| p.axis(**a) != 0)
            .map(|a| format!("{}={}", a.name(), p.axis(*a)))
            .collect();
        out += &format!("P{}: [{}] {}   ", i + 1, pressed.join(" "), axes.join(" "));
    }
    if out.is_empty() { "(idle)".into() } else { out }
}

fn input_test(profile: &Profile) -> Result<()> {
    let stop = stop_flag()?;
    let mut hub = input::Hub::new(profile)?;
    eprintln!("input-test: press buttons, Ctrl+C to quit");
    let mut last = InputFrame::default();
    while !stop.load(Ordering::SeqCst) {
        hub.poll(Duration::from_millis(10));
        let frame = hub.frame();
        if frame != last {
            println!("{}", describe(&frame));
            last = frame;
        }
        if hub.exit_requested {
            println!("(exit combo pressed)");
            hub.exit_requested = false;
        }
    }
    Ok(())
}

/// The gamescope of a headless run: the profile's screen size (else 1920x1080, the Batocera
/// box's), its scaler, no window or display output.
fn headless_gamescope(conf: &config::GamescopeConfig) -> config::GamescopeConfig {
    let (width, height) = if conf.enabled && conf.width > 0 && conf.height > 0 { (conf.width, conf.height) } else { (1920, 1080) };
    let mut args: Vec<String> = if conf.enabled { conf.args.iter().filter(|a| *a != "-f").cloned().collect() } else { Vec::new() };
    args.extend(["--backend".to_string(), "headless".to_string()]);
    config::GamescopeConfig { enabled: true, bin: conf.bin.clone(), width, height, args }
}

/// The X screen's size (`xrandr`'s current size), none without one.
fn screen_size() -> Option<(u32, u32)> {
    let out = Command::new("xrandr").arg("--current").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "Screen 0: minimum 8 x 8, current 1600 x 1200, maximum ..."
    let current = text.lines().next()?.split(", ").find_map(|p| p.strip_prefix("current "))?;
    let (w, h) = current.split_once(" x ")?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// `arg` with `{screen_width}` / `{screen_height}` (the screen's size), `{picture_width}` /
/// `{picture_height}` (the largest centered `WAL_D3D9_ASPECT` rectangle of the screen, the whole
/// screen without it or with `WAL_KEEP_ASPECT_RATIO=0`) and `{aspect_width}` / `{aspect_height}`
/// (that rectangle whatever `WAL_KEEP_ASPECT_RATIO`) replaced. For players sized from their
/// command line: PGA TOUR Team Challenge's Anark frontend draws its 800x600 presentation at its
/// `-pwidth` / `-pheight` (no gamescope). The profile's environment values too: Justice League:
/// Heroes United's screen size in `WAL_INI_KEYS`.
fn screen_placeholders(arg: &str, (sw, sh): (u32, u32), env: &std::collections::BTreeMap<String, String>) -> String {
    if !arg.contains('{') {
        return arg.to_string();
    }
    let keep = !env.get("WAL_KEEP_ASPECT_RATIO").is_some_and(|v| v.trim() == "0");
    let aspect = env.get("WAL_D3D9_ASPECT").and_then(|v| {
        let (w, h) = v.split_once(':')?;
        Some((w.trim().parse::<u64>().ok()?, h.trim().parse::<u64>().ok()?))
    });
    let fit = |aspect: Option<(u64, u64)>| match aspect.filter(|&(w, h)| w > 0 && h > 0) {
        Some((aw, ah)) if sw as u64 * ah > sh as u64 * aw => ((sh as u64 * aw / ah) as u32, sh),
        Some((aw, ah)) => (sw, (sw as u64 * ah / aw) as u32),
        None => (sw, sh),
    };
    let (pw, ph) = fit(aspect.filter(|_| keep));
    let (aw, ah) = fit(aspect);
    arg.replace("{screen_width}", &sw.to_string())
        .replace("{aspect_width}", &aw.to_string())
        .replace("{aspect_height}", &ah.to_string())
        .replace("{screen_height}", &sh.to_string())
        .replace("{picture_width}", &pw.to_string())
        .replace("{picture_height}", &ph.to_string())
}

/// The gamescope arguments with the `fit` scaler (`-S fit`, `--scaler fit`) made `stretch`
/// when `stretch` (the profile's `WAL_KEEP_ASPECT_RATIO=0`, Batocera ES's KEEP ASPECT RATIO
/// off) and the game's screen is not 4:3: wide games fill a 4:3 screen, 4:3 games (Beach Head's
/// 640x480 shells, Need for Speed's 800x600) keep their aspect, black bars on a wide screen.
fn stretched(conf: &config::GamescopeConfig, stretch: bool) -> config::GamescopeConfig {
    let mut args = conf.args.clone();
    let four_three = conf.width > 0 && conf.width * 3 == conf.height * 4;
    if stretch && !four_three {
        for i in 1..args.len() {
            if matches!(args[i - 1].as_str(), "-S" | "--scaler") && args[i] == "fit" {
                args[i] = "stretch".to_string();
            }
        }
    }
    config::GamescopeConfig { enabled: conf.enabled, bin: conf.bin.clone(), width: conf.width, height: conf.height, args }
}

/// `cmd` run inside gamescope, with its environment and directory.
/// `cmd` started by a shell after the `background` programs (left running) and `wait_ms`.
fn with_background(cmd: Command, background: Vec<Command>, wait_ms: u64) -> Command {
    let quote = |s: &std::ffi::OsStr| format!("'{}'", s.to_string_lossy().replace('\'', "'\\''"));
    let mut script = String::new();
    for bg in &background {
        if let Some(dir) = bg.get_current_dir() {
            script += &format!("(cd {} && ", quote(dir.as_os_str()));
        } else {
            script += "(";
        }
        script += &std::iter::once(bg.get_program()).chain(bg.get_args()).map(quote).collect::<Vec<_>>().join(" ");
        script += ") & ";
    }
    // the game's lines appended to the payload log the background program created
    script += &format!("sleep {}; WAL_LOG_APPEND=1 exec \"$@\"", wait_ms as f64 / 1000.0);
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(script).arg("sh").arg(cmd.get_program()).args(cmd.get_args());
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => sh.env(k, v),
            None => sh.env_remove(k),
        };
    }
    if let Some(dir) = cmd.get_current_dir() {
        sh.current_dir(dir);
    }
    sh
}

fn gamescope(conf: &config::GamescopeConfig, cmd: Command) -> Command {
    let mut gs = Command::new(&conf.bin);
    if conf.width > 0 && conf.height > 0 {
        gs.arg("-w").arg(conf.width.to_string()).arg("-h").arg(conf.height.to_string());
    }
    gs.args(&conf.args).arg("--").arg(cmd.get_program()).args(cmd.get_args());
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => gs.env(k, v),
            None => gs.env_remove(k),
        };
    }
    if let Some(dir) = cmd.get_current_dir() {
        gs.current_dir(dir);
    }
    gs
}

/// Open files limit of the game (wine with ntsync: a descriptor per synchronization object).
/// Batocera starts games with 1024 (4096 hard): Crimzon Clover (DxLib) could not start its
/// loading threads and quit. The kernel's maximum (fs.nr_open) as root, else the hard limit.
fn raise_open_files_limit() {
    let wanted: libc::rlim_t = std::fs::read_to_string("/proc/sys/fs/nr_open")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1_048_576);
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 || lim.rlim_cur >= wanted {
        return;
    }
    let before = lim.rlim_cur;
    if lim.rlim_max < wanted {
        let raised = libc::rlimit { rlim_cur: wanted, rlim_max: wanted };
        // root only (CAP_SYS_RESOURCE)
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
            eprintln!("launcher: open files limit {before} -> {wanted}");
            return;
        }
    }
    lim.rlim_cur = lim.rlim_max;
    if lim.rlim_cur > before && unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } == 0 {
        eprintln!("launcher: open files limit {before} -> {}", lim.rlim_cur);
    }
}

/// Launch options of `run` (command line).
struct RunOptions {
    dry_run: bool,
    /// Only the input script drives the game.
    no_input: bool,
    /// The game in a headless gamescope.
    headless: bool,
}

fn run(profile: &Profile, opts: &RunOptions, script: Option<script::Script>) -> Result<()> {
    let border = sinden::select(profile)?;
    let dry_run = opts.dry_run;
    raise_open_files_limit();
    if profile.exe.is_empty() {
        bail!("{}: launch a game from its dump (directory or <gameid>.windowsloader file)\n{USAGE}", profile.id);
    }
    let system = systems::by_name(&profile.system)?;
    let mut wine = wine::Wine::new(profile, dry_run)?;

    let exe = wine.unix_path(&profile.exe)?;
    if !exe.is_file() {
        bail!("game executable not found: {} ({})", profile.exe, exe.display());
    }
    let game_dir = exe.parent().context("executable without directory")?;
    let exe_name = exe.file_name().context("executable without name")?;
    let mut game_root = game_dir;
    for _ in 0..profile.exe_depth {
        game_root = game_root.parent().context("exe_depth goes above the filesystem root")?;
    }
    // path of the executable folder inside the game root
    let exe_subdir = game_dir.strip_prefix(game_root)?;

    let payload_dir = profile.path(&profile.payloads_dir);
    let system_payloads = match system.payloads64() {
        Some(p) if rundir::is_pe64(&exe) => p,
        _ => system.payloads(),
    };
    let mut payloads: Vec<(PathBuf, &str)> = match system_payloads.first() {
        // the profile's install paths, for the system's payload file
        Some(p) if !profile.payload_install.is_empty() => {
            profile.payload_install.iter().map(|path| (payload_dir.join(p.file), path.as_str())).collect()
        }
        _ => system_payloads.iter().map(|p| (payload_dir.join(p.file), p.install_as)).collect(),
    };
    // A system may have no payload for one executable architecture. In that case run the game
    // directly instead of starting wal-loader with a payload from the other architecture.
    // wal-loader is built for 32-bit games, and for the 64-bit games of `payload_loader`
    // (wal-loader64.exe: Disney Crossy Road, whose screen Unity reads before it loads the
    // payload). Other 64-bit payloads (Namco ES3 Battle Pod) are loaded through the game's own
    // imports.
    let loader = if system_payloads.is_empty() {
        None
    } else if profile.payload_loader {
        payloads.first().map(|(_, at)| *at)
    } else if rundir::is_pe64(&exe) {
        None
    } else if !profile.payload_install.is_empty() {
        // the game imports the payload under the profile's names
        None
    } else {
        system.loader()
    };
    // the payload of the programs a relaunch starts (`exe=`), when the game's own is imported
    let program_loader = if loader.is_none() && !rundir::is_pe64(&exe) { system.program_loader() } else { None };
    if loader.is_some() || program_loader.is_some() {
        let file = if rundir::is_pe64(&exe) { "wal-loader64.exe" } else { "wal-loader.exe" };
        payloads.push((payload_dir.join(file), "wal-loader.exe"));
    }
    // a background program of the other architecture (CHUNITHM SUPERSTAR: a 32-bit game, its
    // 64-bit amdaemon): that architecture's loader and payload too, under their own names
    let game64 = rundir::is_pe64(&exe);
    let (other_loader, other_payload) = if game64 { ("wal-loader32.exe", "wal_payload32.dll") } else { ("wal-loader64.exe", "wal_payload64.dll") };
    let other_arch = loader.is_some()
        && profile.background.iter().any(|b| b.split_whitespace().next().is_some_and(|e| rundir::is_pe64(&game_dir.join(e)) != game64));
    if other_arch {
        let other_payloads = if game64 { system.payloads() } else { system.payloads64().unwrap_or(&[]) };
        if let Some(p) = other_payloads.first() {
            payloads.push((payload_dir.join(p.file), other_payload));
            payloads.push((payload_dir.join(if game64 { "wal-loader.exe" } else { "wal-loader64.exe" }), other_loader));
        }
    }
    // a game DLL (Konami AVS): run by the system's host program
    let dll_host = if exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
        let host = system.dll_host().with_context(|| format!("{}: a DLL, and system {} runs no game DLL", profile.exe, system.name()))?;
        // a 64-bit game DLL: the host's 64-bit build (`<host>64.exe`), installed under its name
        let file = match host.strip_suffix(".exe") {
            Some(stem) if rundir::is_pe64(&exe) => format!("{stem}64.exe"),
            _ => host.to_string(),
        };
        payloads.push((payload_dir.join(file), host));
        Some(host)
    } else {
        None
    };
    if let Some(p) = program_loader {
        payloads.push((payload_dir.join(p), p));
    }
    for (src, _) in &payloads {
        if !src.exists() {
            bail!("payload {} missing: build it with ./build.sh", src.display());
        }
    }
    // profile files are installed like the payloads: copied in place of the game's file
    for (name, src) in &profile.files {
        let src = match (src.strip_prefix("{dump}"), src.strip_prefix("{payloads}"), src.strip_prefix("{runner}")) {
            (Ok(rel), _, _) => game_root.join(rel),
            // a program built with the payloads (a dongle-wrapped executable's replacement)
            (_, Ok(file), _) => payload_dir.join(file),
            // a component the wine runner ships (gxvk's Glide DLLs)
            (_, _, Ok(rel)) => wine.runner.join(rel),
            _ => profile.path(src),
        };
        if !src.is_file() {
            bail!("profile file {} missing", src.display());
        }
        payloads.push((src, name.as_str()));
    }

    wine.prepare_prefix()?;
    wine.map_game_drives(&profile.drives, game_root)?;
    wine.apply_tricks(&profile.prefix_tricks)?;
    wine.apply_tricks(&profile.tricks)?;
    let keep: Vec<&str> = profile.dll_overrides.keys().map(|d| d.trim_start_matches('*')).collect();
    wine.builtin_unlisted_tricks(&profile.prefix_tricks, &profile.tricks, &keep);
    for (dll, mode) in &profile.dll_overrides {
        wine.override_dll(&format!("{dll}={mode}"));
    }
    // a ReShade setup is used when the dump has all of its files (dumps without one, or with
    // only some of them, run the game's own picture)
    let use_reshade = profile.reshade && {
        let names: Vec<String> = std::fs::read_dir(&game_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_ascii_lowercase())
            .collect();
        let missing: Vec<&String> =
            profile.reshade_files.iter().filter(|f| !names.contains(&f.to_ascii_lowercase())).collect();
        if !missing.is_empty() && missing.len() < profile.reshade_files.len() {
            eprintln!("launcher: ReShade setup incomplete ({missing:?} missing): not used");
        }
        missing.is_empty()
    };
    let reshade: &[String] = if use_reshade { &profile.reshade_files } else { &[] };
    wine.setup_d3d(profile.dxvk, reshade)?;
    wine.setup_ddraw(profile.graphics)?;

    let mut hide: Vec<String> = profile.hide.clone();
    if !use_reshade {
        hide.extend(profile.reshade_files.iter().cloned());
    }
    hide.extend(system.hidden().iter().map(|s| s.to_string()));
    if matches!(profile.graphics, Graphics::Wine | Graphics::D7vk) {
        // dgVoodoo shipped with the game would take precedence
        hide.extend(["ddraw.dll".into(), "d3dimm.dll".into()]);
    }
    let run_dir = wine.prefix.join("drive_c/wal").join(system.name()).join(profile.slug());
    // payload names: next to the executable, or a path from the game root ("dir/file.dll")
    let payload_refs: Vec<(&Path, PathBuf)> = payloads
        .iter()
        .map(|(p, n)| {
            let at = if n.contains(['/', '\\']) {
                n.split(['/', '\\']).filter(|c| !c.is_empty()).collect()
            } else {
                exe_subdir.join(n)
            };
            (p.as_path(), at)
        })
        .collect();
    if !dry_run {
        for dir in system.data_dirs() {
            rundir::migrate_data_dir(game_dir, dir)?;
            std::fs::create_dir_all(game_dir.join(dir))
                .with_context(|| format!("creating {}", game_dir.join(dir).display()))?;
        }
        for (name, src) in &profile.initial_files {
            let dst = game_root.join(name);
            if !dst.exists() {
                if let Some(dir) = dst.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let src = match src.strip_prefix("{dump}") {
                    Ok(rel) => game_root.join(rel),
                    Err(_) => profile.path(src),
                };
                std::fs::copy(&src, &dst).with_context(|| format!("initial file {} -> {}", src.display(), dst.display()))?;
                eprintln!("launcher: {name} installed in the dump");
            }
        }
        // folders on the way to the executable and to the payloads are real directories, the
        // rest are links
        rundir::build_tree(&run_dir, game_root, &hide, &[exe_subdir.to_path_buf()], &payload_refs)?;
        if profile.exe_fixed_base {
            rundir::copy_fixed_base(&exe, &run_dir.join(exe_subdir).join(exe_name))?;
        } else if profile.large_address_aware {
            rundir::copy_large_address_aware(&exe, &run_dir.join(exe_subdir).join(exe_name))?;
        }
        for (file, sections) in &profile.readonly_sections {
            rundir::readonly_sections(&game_root.join(file), &run_dir.join(file), sections)?;
        }
    }
    wine.map_c_folders(&profile.c_folders, &run_dir, game_root)?;
    wine.map_run_drives(&profile.run_drives, &run_dir)?;
    wine.link_game_fonts(&profile.fonts, game_root)?;
    wine.import_registry(&profile.registry.iter().map(|f| profile.path(f)).collect::<Vec<_>>())?;
    let run_exe_dir = run_dir.join(exe_subdir);

    wine.set_env(env::PORT, &profile.port.to_string());
    wine.set_env(env::LOG, &wine.windows_path(&run_dir.join(format!("wal-{}.log", system.name()))));
    if !profile.native_map.is_empty() {
        let map: Vec<String> = profile.native_map.iter().map(|(k, v)| format!("{k}={v}")).collect();
        wine.set_env(env::MAP, &map.join(","));
    }
    // the player's NESiCA card (GUI), before the profile's own values
    if matches!(system.name(), "nesica" | "nesica2") {
        match nesica_card::load(&profile.root) {
            Ok(Some(card)) => nesica_card::env(&card).into_iter().for_each(|(k, v)| wine.set_env(k, &v)),
            Ok(None) => {}
            Err(e) => eprintln!("launcher: {e:#}"),
        }
    }
    // the player's e-Amusement card (GUI), before the profile's own values
    if system.name() == "konami" {
        match eamuse_card::load(&profile.root) {
            Ok(Some(card)) => eamuse_card::env(&card).into_iter().for_each(|(k, v)| wine.set_env(k, &v)),
            Ok(None) => {}
            Err(e) => eprintln!("launcher: {e:#}"),
        }
    }
    // the screen's size placeholders of the arguments and the environment (headless:
    // gamescope's screen)
    let screen = if opts.headless {
        let conf = headless_gamescope(&profile.gamescope);
        (conf.width, conf.height)
    } else {
        screen_size().unwrap_or((1920, 1080))
    };
    for (k, v) in &profile.env {
        wine.set_env(k, &screen_placeholders(v, screen, &profile.env));
    }

    wine.set_env("WAL_SINDEN_BORDER", if border.is_some() { "1" } else { "0" });
    if let Some(border) = border {
        // Physical geometry is shared with the renderer even if a game changes its mode.
        wine.set_env("WAL_SINDEN_SCREEN", &format!("{}x{}", screen.0, screen.1));
        eprintln!("sinden: border {}% color #{:06x}", border.percent, border.rgb);
    }
    let game_exe = wine.windows_path(&run_exe_dir.join(exe_name));
    let screen_args = |args: &[String]| -> Vec<String> { args.iter().map(|a| screen_placeholders(a, screen, &profile.env)).collect() };
    // a program of the profile's `background` list (its command line from the game directory)
    let background_command = |program: &str| -> Option<Command> {
        let words: Vec<String> = screen_args(&program.split(' ').filter(|w| !w.is_empty()).map(String::from).collect::<Vec<_>>());
        let (exe, args) = words.split_first()?;
        let exe_path = wine.windows_path(&run_exe_dir.join(exe));
        // the other architecture's loader and payload for a program of it
        let other = other_arch && rundir::is_pe64(&run_exe_dir.join(exe)) != game64;
        let mut cmd = match loader {
            Some(_) if other => {
                let mut cmd = wine.command(wine.windows_path(&run_exe_dir.join(other_loader)));
                cmd.arg(wine.windows_path(&run_exe_dir.join(other_payload))).arg(&exe_path);
                cmd
            }
            Some(payload) => {
                let mut cmd = wine.command(wine.windows_path(&run_exe_dir.join("wal-loader.exe")));
                let at = if payload.contains(['/', '\\']) { run_dir.join(payload) } else { run_exe_dir.join(payload) };
                cmd.arg(wine.windows_path(&at)).arg(&exe_path);
                cmd
            }
            None => wine.command(&exe_path),
        };
        // from its own folder (game.bat's `pushd amdaemon`: its configuration files there)
        let cwd = run_exe_dir.join(exe).parent().map_or_else(|| run_exe_dir.clone(), |d| d.to_path_buf());
        cmd.args(args).current_dir(cwd);
        Some(cmd)
    };
    // the game command with these arguments; `relaunched`: started again at the payload's
    // request (WAL_RELAUNCH=1)
    // `exe`: another executable of the game directory (a relaunch to the test menu program)
    let game_command = |args: &[String], relaunched: bool, exe: Option<&str>| {
        let args = screen_args(args);
        let args = args.as_slice();
        let game_exe = exe.map_or_else(|| game_exe.clone(), |e| wine.windows_path(&run_exe_dir.join(e)));
        let mut cmd = match loader.or(exe.and(program_loader)) {
            // wal-loader <payload> <game> [args]: payload loaded before the game entry point
            Some(payload) => {
                let mut cmd = wine.command(wine.windows_path(&run_exe_dir.join("wal-loader.exe")));
                // a payload path from the game root ("dir/file.dll"), else next to the executable
                let at = if payload.contains(['/', '\\']) { run_dir.join(payload) } else { run_exe_dir.join(payload) };
                cmd.arg(wine.windows_path(&at)).arg(&game_exe);
                cmd
            }
            None => match dll_host.filter(|_| exe.is_none()) {
                Some(host) => {
                    let mut cmd = wine.command(wine.windows_path(&run_exe_dir.join(host)));
                    cmd.arg(exe_name);
                    cmd
                }
                None => wine.command(&game_exe),
            },
        };
        cmd.args(args).current_dir(&run_exe_dir);
        if relaunched {
            cmd.env(env::RELAUNCH, "1");
        }
        // KEEP ASPECT RATIO off: gamescope's fit scaler stretches instead
        let stretch = profile.env.get("WAL_KEEP_ASPECT_RATIO").is_some_and(|v| v.trim() == "0");
        if opts.headless {
            // the background programs in the same gamescope (one X display: a Wine call crossing
            // two, e.g. ClipCursor, ends the game with an X BadWindow error)
            if !relaunched && !profile.background.is_empty() {
                cmd = with_background(cmd, profile.background.iter().filter_map(|p| background_command(p)).collect(), profile.background_wait_ms);
            }
            cmd = gamescope(&stretched(&headless_gamescope(&profile.gamescope), stretch), cmd);
        } else if profile.gamescope.enabled {
            cmd = gamescope(&stretched(&profile.gamescope, stretch), cmd);
        }
        // own process group: whatever the game leaves (gamescope's reaper and its children) is
        // killed with it at the end
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        cmd
    };
    let mut cmd = game_command(&profile.args, false, None);

    eprintln!("game: {} ({})", profile.name.as_deref().unwrap_or(&profile.id), profile.id);
    eprintln!("profile layers: {:?}", profile.sources);
    eprintln!("game directory: {}", game_dir.display());
    eprintln!("run directory: {}", run_dir.display());
    if dry_run {
        for (k, v) in wine.env() {
            println!("{k}={v}");
        }
        let argv: Vec<_> = std::iter::once(cmd.get_program()).chain(cmd.get_args()).collect();
        println!("cd {:?} && {:?}", run_exe_dir, argv);
        return Ok(());
    }

    let stop = stop_flag()?;
    let server = server::Server::start(profile.port)?;
    let mut hub = input::Hub::new(profile)?;
    hub.border = border.map(|b| (b, [screen.0, screen.1]));
    for helper in &profile.pre_launch {
        let helper_path = run_exe_dir.join(helper);
        let helper_path = wine.windows_path(&helper_path);
        eprintln!("launcher: running pre-launch helper {helper}");
        let status = wine
            .command(&helper_path)
            .current_dir(&run_exe_dir)
            .status()
            .with_context(|| format!("running pre-launch helper {helper}"))?;
        if !status.success() {
            bail!("pre-launch helper {helper} failed: {status}");
        }
    }
    // programs left running beside the game (Sega ALLS's amdaemon.exe), with the game's payload
    // when the system starts it through wal-loader; ended with the game's leftovers (headless:
    // started inside the game's gamescope instead)
    if !opts.headless {
        for mut cmd in profile.background.iter().filter_map(|p| background_command(p)) {
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
            eprintln!("launcher: starting background program {:?}", cmd.get_program());
            cmd.spawn().with_context(|| format!("starting background program {:?}", cmd.get_program()))?;
        }
    }
    if !profile.background.is_empty() && !opts.headless {
        std::thread::sleep(Duration::from_millis(profile.background_wait_ms));
        // the payload log the background program created: the game's lines appended to it
        cmd.env("WAL_LOG_APPEND", "1");
    }
    let mut child = cmd.spawn().context("starting wine")?;
    // its window to the front (a frontend keeping the focus hid it)
    let mut focus = if opts.headless { None } else { focus::Focus::new(child.id()) };

    let mut last = InputFrame::default();
    let mut last_sent = Instant::now();
    // the touch surface's fingers (`input.touch`)
    let touches = (profile.input.touch && !opts.no_input).then(touch::start);
    let mut last_touches: Vec<(u16, u16)> = Vec::new();
    let mut killing = false;
    // when the exit was asked, until the game's process group is killed itself
    let mut kill_asked: Option<Instant> = None;
    let mut last_check = Instant::now();
    let status = loop {
        hub.poll(Duration::from_millis(4));
        if let Some(f) = &mut focus {
            f.poll();
        }
        let mut frame = if opts.no_input { InputFrame::default() } else { hub.frame() };
        if opts.no_input {
            hub.exit_requested = false;
        }
        if let Some(script) = &script {
            for (p, stick) in frame.players.iter_mut().enumerate() {
                script.apply(p, stick);
            }
        }
        // force feedback outputs of the game, to the players' devices
        for o in server.outputs() {
            hub.output(o);
        }
        // on change, plus a periodic refresh
        if frame != last || last_sent.elapsed() > Duration::from_millis(100) {
            server.broadcast(&frame);
            last = frame;
            last_sent = Instant::now();
            if touches.is_some() {
                server.broadcast_touch(&last_touches);
            }
        }
        if let Some(t) = &touches {
            let points = t.get();
            if points != last_touches {
                server.broadcast_touch(&points);
                last_touches = points;
            }
        }
        if (hub.exit_requested || stop.load(Ordering::SeqCst)) && !killing {
            eprintln!("launcher: stopping the game");
            killing = true;
            kill_asked = Some(Instant::now());
            let _ = wine.wineserver("-k");
        }
        // the game still there: `wineserver -k` found another server (the prefix extracted
        // again while the game ran: the server folder is named after the prefix directory's
        // inode, the game's server stayed in the old one)
        if kill_asked.is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
            eprintln!("launcher: the game still runs 5 s after wineserver -k: its processes killed");
            kill_asked = None;
            let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(std::process::Stdio::null()).status();
        }
        if let Some(status) = child.try_wait()? {
            // a game ending to be started again with other arguments (The Swarm: each stage
            // is a run of the game)
            if let Some(args) = server.take_relaunch().filter(|_| !killing) {
                eprintln!("launcher: game exited ({status}), started again: {args}");
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(std::process::Stdio::null()).status();
                // `exe=<file> [cwd=<dir>] args`: that executable of the game directory, with only
                // these arguments (the test menu program), run from that folder of the game
                // directory (its batch file's pushd: Virtua Fighter eSports' vf5fs\vfes.exe);
                // otherwise the game with its own plus these
                let words: Vec<&str> = args.split(' ').filter(|a| !a.is_empty()).collect();
                let (exe, cwd, args): (Option<&str>, Option<&str>, Vec<String>) = match words.first().and_then(|w| w.strip_prefix("exe=")) {
                    Some(exe) => {
                        let cwd = words.get(1).and_then(|w| w.strip_prefix("cwd="));
                        (Some(exe), cwd, words[1 + cwd.is_some() as usize..].iter().map(|a| a.to_string()).collect())
                    }
                    None => (None, None, profile.args.iter().cloned().chain(words.iter().map(|a| a.to_string())).collect()),
                };
                let mut cmd = game_command(&args, true, exe);
                if let Some(cwd) = cwd {
                    cmd.current_dir(run_exe_dir.join(cwd));
                }
                child = cmd.spawn().context("starting wine")?;
                focus = if opts.headless { None } else { focus::Focus::new(child.id()) };
                continue;
            }
            break status;
        }
        // a crash (e.g. a stack overflow under WoW64) can end the game's main thread only:
        // the process stays a zombie leader with live threads, never reaped
        if !killing && last_check.elapsed() > Duration::from_millis(500) {
            last_check = Instant::now();
            if main_thread_dead(child.id()) && server.relaunch_pending() {
                // the game ending to start the next program (a Unity game's main thread ends
                // first: Goonya Fighter's test menu): its leftover threads killed, the relaunch
                // follows
                eprintln!("launcher: the game's main thread is gone (relaunch asked): ending it");
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(std::process::Stdio::null()).status();
            } else if main_thread_dead(child.id()) {
                eprintln!("launcher: the game's main thread is gone (crash): stopping the game");
                killing = true;
                let _ = wine.wineserver("-k");
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(std::process::Stdio::null()).status();
            }
        }
    };
    eprintln!("launcher: game exited ({status})");
    // leftover processes of the game (helpers, services)
    let _ = wine.wineserver("-k");
    let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(std::process::Stdio::null()).status();
    Ok(())
}
