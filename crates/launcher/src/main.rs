//! Windows arcade game launcher for Linux.
//!
//! Reads physical inputs (SDL3 gamepads/wheels, evdev keyboards), maps them to virtual
//! arcade sticks, serves them over TCP to the payload DLL running inside the game, and
//! runs the game with a wine runner.

mod config;
mod guns;
mod input;
mod mapping;
mod rundir;
mod script;
mod server;
mod systems;
mod wine;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use config::{Graphics, Profile};
use wal_protocol::{InputFrame, env};

const USAGE: &str = "\
usage:
  arcade-launcher [run] <dump> [--root DIR] [--dry-run] [--input-script FILE]
      Launch a game. <dump> is a game dump directory holding a <gameid>.windowsloader
      file (the executable path relative to the dump root), or that file. The game id
      selects systemprofiles/<system>/<gameid>.yaml, merged with userprofiles/.
      --root: where systemprofiles/ is (default: the current directory, else next to
      the launcher). --input-script replays timed virtual stick inputs
      (`<seconds> p<N> <inputs...>` per line, `-` releases), for tests.
  arcade-launcher input-test [<dump> | <gameid>] [--root DIR]
      Print the virtual arcade sticks while you press buttons.
  arcade-launcher show <dump> | <gameid> [--root DIR]
      Print the merged profile.";

struct Args {
    command: String,
    profile: Option<String>,
    root: Option<PathBuf>,
    dry_run: bool,
    script: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut args = Args { command: "run".into(), profile: None, root: None, dry_run: false, script: None };
    let mut positional = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => args.root = Some(it.next().context("--root needs a directory")?.into()),
            "--dry-run" => args.dry_run = true,
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
        Some(c) if ["run", "input-test", "show"].contains(&c.as_str()) => {
            args.command = c;
            args.profile = positional.next();
        }
        other => args.profile = other,
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
    let load = || -> Result<Profile> {
        let p = args.profile.as_deref().with_context(|| format!("missing <dump>\n{USAGE}"))?;
        Profile::load(p, args.root.as_deref())
    };
    match args.command.as_str() {
        "show" => {
            let p = load()?;
            println!("# {} from {:?}\n{p:#?}", p.id, p.sources);
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
            run(&load()?, args.dry_run, script)
        }
    }
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

fn run(profile: &Profile, dry_run: bool, script: Option<script::Script>) -> Result<()> {
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
    let mut payloads: Vec<(PathBuf, &str)> =
        system.payloads().iter().map(|p| (payload_dir.join(p.file), p.install_as)).collect();
    if system.loader().is_some() {
        payloads.push((payload_dir.join("wal-loader.exe"), "wal-loader.exe"));
    }
    for (src, _) in &payloads {
        if !src.exists() {
            bail!("payload {} missing: build it with ./build.sh", src.display());
        }
    }
    // profile files are installed like the payloads: copied in place of the game's file
    for (name, src) in &profile.files {
        let src = profile.path(src);
        if !src.is_file() {
            bail!("profile file {} missing", src.display());
        }
        payloads.push((src, name.as_str()));
    }

    wine.prepare_prefix()?;
    wine.apply_tricks(&profile.tricks)?;
    let reshade: &[String] = if profile.reshade { &profile.reshade_files } else { &[] };
    wine.setup_d3d(profile.dxvk, reshade)?;
    wine.setup_ddraw(profile.graphics)?;

    let mut hide: Vec<String> = profile.hide.clone();
    if !profile.reshade {
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
        // folders on the way to the executable and to the payloads are real directories, the
        // rest are links
        rundir::build_tree(&run_dir, game_root, &hide, &[exe_subdir.to_path_buf()], &payload_refs)?;
    }
    let run_exe_dir = run_dir.join(exe_subdir);

    wine.set_env(env::PORT, &profile.port.to_string());
    wine.set_env(env::LOG, &wine.windows_path(&run_dir.join(format!("wal-{}.log", system.name()))));
    if !profile.native_map.is_empty() {
        let map: Vec<String> = profile.native_map.iter().map(|(k, v)| format!("{k}={v}")).collect();
        wine.set_env(env::MAP, &map.join(","));
    }
    for (k, v) in &profile.env {
        wine.set_env(k, v);
    }

    let game_exe = wine.windows_path(&run_exe_dir.join(exe_name));
    let mut cmd = match system.loader() {
        // wal-loader <payload> <game> [args]: payload loaded before the game entry point
        Some(payload) => {
            let mut cmd = wine.command(wine.windows_path(&run_exe_dir.join("wal-loader.exe")));
            cmd.arg(wine.windows_path(&run_exe_dir.join(payload))).arg(&game_exe);
            cmd
        }
        None => wine.command(&game_exe),
    };
    cmd.args(&profile.args).current_dir(&run_exe_dir);

    eprintln!("game: {} ({})", profile.name.as_deref().unwrap_or(&profile.id), profile.id);
    eprintln!("profile layers: {:?}", profile.sources);
    eprintln!("game directory: {}", game_dir.display());
    eprintln!("run directory: {}", run_dir.display());
    if dry_run {
        for (k, v) in wine.env() {
            println!("{k}={v}");
        }
        println!("cd {:?} && {:?} {:?} {:?}", run_exe_dir, wine.runner.join("bin/wine"), game_exe, profile.args);
        return Ok(());
    }

    let stop = stop_flag()?;
    let server = server::Server::start(profile.port)?;
    let mut hub = input::Hub::new(profile)?;
    let mut child = cmd.spawn().context("starting wine")?;

    let mut last = InputFrame::default();
    let mut last_sent = Instant::now();
    let mut killing = false;
    let status = loop {
        hub.poll(Duration::from_millis(4));
        let mut frame = hub.frame();
        if let Some(script) = &script {
            for (p, stick) in frame.players.iter_mut().enumerate() {
                stick.buttons |= script.buttons(p);
            }
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
            break status;
        }
    };
    eprintln!("launcher: game exited ({status})");
    // leftover processes of the game (helpers, services)
    let _ = wine.wineserver("-k");
    Ok(())
}
