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
mod squashfs;
mod systems;
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

/// `cmd` run inside gamescope, with its environment and directory.
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
    // wal-loader is built for 32-bit games. A system may also have 64-bit payloads
    // (Namco ES3 Battle Pod), which are loaded through the game's own imports.
    let loader = if system_payloads.is_empty() || rundir::is_pe64(&exe) {
        None
    } else if profile.payload_loader {
        payloads.first().map(|(_, at)| *at)
    } else if !profile.payload_install.is_empty() {
        // the game imports the payload under the profile's names
        None
    } else {
        system.loader()
    };
    // the payload of the programs a relaunch starts (`exe=`), when the game's own is imported
    let program_loader = if loader.is_none() && !rundir::is_pe64(&exe) { system.program_loader() } else { None };
    if loader.is_some() || program_loader.is_some() {
        payloads.push((payload_dir.join("wal-loader.exe"), "wal-loader.exe"));
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
        let src = match src.strip_prefix("{dump}") {
            Ok(rel) => game_root.join(rel),
            Err(_) => profile.path(src),
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
    wine.map_c_folders(&profile.c_folders, &run_dir)?;
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
    for (k, v) in &profile.env {
        wine.set_env(k, v);
    }

    let game_exe = wine.windows_path(&run_exe_dir.join(exe_name));
    // the game command with these arguments; `relaunched`: started again at the payload's
    // request (WAL_RELAUNCH=1)
    // `exe`: another executable of the game directory (a relaunch to the test menu program)
    let game_command = |args: &[String], relaunched: bool, exe: Option<&str>| {
        let game_exe = exe.map_or_else(|| game_exe.clone(), |e| wine.windows_path(&run_exe_dir.join(e)));
        let mut cmd = match loader.or(exe.and(program_loader)) {
            // wal-loader <payload> <game> [args]: payload loaded before the game entry point
            Some(payload) => {
                let mut cmd = wine.command(wine.windows_path(&run_exe_dir.join("wal-loader.exe")));
                cmd.arg(wine.windows_path(&run_exe_dir.join(payload))).arg(&game_exe);
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
        if opts.headless {
            cmd = gamescope(&headless_gamescope(&profile.gamescope), cmd);
        } else if profile.gamescope.enabled {
            cmd = gamescope(&profile.gamescope, cmd);
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
    let mut child = cmd.spawn().context("starting wine")?;
    // its window to the front (a frontend keeping the focus hid it)
    let mut focus = if opts.headless { None } else { focus::Focus::new(child.id()) };

    let mut last = InputFrame::default();
    let mut last_sent = Instant::now();
    let mut killing = false;
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
        }
        if (hub.exit_requested || stop.load(Ordering::SeqCst)) && !killing {
            eprintln!("launcher: stopping the game");
            killing = true;
            let _ = wine.wineserver("-k");
        }
        if let Some(status) = child.try_wait()? {
            // a game ending to be started again with other arguments (The Swarm: each stage
            // is a run of the game)
            if let Some(args) = server.take_relaunch().filter(|_| !killing) {
                eprintln!("launcher: game exited ({status}), started again: {args}");
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).stderr(std::process::Stdio::null()).status();
                // `exe=<file> args`: that executable of the game directory, with only these
                // arguments (the test menu program); otherwise the game with its own plus these
                let words: Vec<&str> = args.split(' ').filter(|a| !a.is_empty()).collect();
                let (exe, args): (Option<&str>, Vec<String>) = match words.first().and_then(|w| w.strip_prefix("exe=")) {
                    Some(exe) => (Some(exe), words[1..].iter().map(|a| a.to_string()).collect()),
                    None => (None, profile.args.iter().cloned().chain(words.iter().map(|a| a.to_string())).collect()),
                };
                child = game_command(&args, true, exe).spawn().context("starting wine")?;
                focus = if opts.headless { None } else { focus::Focus::new(child.id()) };
                continue;
            }
            break status;
        }
        // a crash (e.g. a stack overflow under WoW64) can end the game's main thread only:
        // the process stays a zombie leader with live threads, never reaped
        if !killing && last_check.elapsed() > Duration::from_millis(500) {
            last_check = Instant::now();
            if main_thread_dead(child.id()) {
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
