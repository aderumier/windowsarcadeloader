//! Frontend window (`arcade-launcher` without a game): every supported game, its controls
//! mapped per device (keyboard, gamepad, wheel) into its user profile, its dump folder, and
//! its launch.

mod bindings;
mod capture;

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use eframe::egui;
use serde_yaml_ng::Value;

use crate::config::{self, DUMP_EXT, Dump, Layers, MapTable, Profile, USER_PROFILES};
use crate::squashfs::{self, DumpFile};
use bindings::{Captured, Device, Row, RowKind};
use capture::InputEvent;

/// `userprofiles/` file of the games' dumps: `<system>/<game>: <folder or SquashFS image>`.
const GAMEDIRS: &str = "gamedirs.yaml";
const LOG_LINES: usize = 400;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(root: &Path) -> Result<()> {
    let root = root.to_path_buf();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Windows Arcade Loader")
            .with_app_id("windows-arcade-loader")
            .with_inner_size([1100.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Windows Arcade Loader",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let wake = ctx.clone();
            Ok(Box::new(App::new(root, capture::start(move || wake.request_repaint()), ctx)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("GUI: {e}"))
}

struct Game {
    /// `<system>/<game>`.
    id: String,
    name: String,
    system: String,
}

impl Game {
    fn slug(&self) -> &str {
        self.id.rsplit('/').next().unwrap_or(&self.id)
    }
}

/// The selected game's profile.
struct Selected {
    layers: Layers,
    profile: Profile,
}

/// A connected SDL device.
struct Connected {
    name: String,
    is_pad: bool,
    axes: Vec<i32>,
}

/// Waiting for an input to bind to a row.
struct Capture {
    device: Device,
    row: Row,
    /// The row's other inputs removed (set), else one more (add).
    replace: bool,
    started: Instant,
    /// Raw joystick axes when the capture started: (device, axis) -> value.
    rest: HashMap<(u32, u8), i32>,
}

/// What a game's dump setting gives.
enum DumpState {
    NotSet,
    /// A folder without the game's `.windowsloader`.
    NoFile,
    /// Its image being read.
    Checking,
    /// `target`: what `run` gets (the `.windowsloader` file or the image).
    Ready { target: PathBuf, exe: String },
    Error(String),
}

type ImageResult = Result<Vec<DumpFile>, String>;

/// SquashFS images' `.windowsloader` files, read in a worker thread.
struct Images {
    results: HashMap<PathBuf, Option<ImageResult>>,
    requests: std::sync::mpsc::Sender<PathBuf>,
    done: Receiver<(PathBuf, ImageResult)>,
}

impl Images {
    fn new(ctx: egui::Context) -> Images {
        let (requests, todo) = std::sync::mpsc::channel::<PathBuf>();
        let (finished, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for image in todo {
                let r = squashfs::dump_files(&image).map_err(|e| format!("{e:#}"));
                if finished.send((image, r)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Images { results: HashMap::new(), requests, done }
    }

    /// The image's files, once read (asked to the worker the first time).
    fn get(&mut self, image: &Path) -> Option<&ImageResult> {
        while let Ok((path, r)) = self.done.try_recv() {
            self.results.insert(path, Some(r));
        }
        if !self.results.contains_key(image) {
            self.results.insert(image.to_path_buf(), None);
            let _ = self.requests.send(image.to_path_buf());
        }
        self.results.get(image).and_then(Option::as_ref)
    }
}

struct Running {
    child: Child,
    game: String,
}

struct App {
    root: PathBuf,
    games: Vec<Game>,
    filter: String,
    /// Only the games whose dump folder is set and holds their `.windowsloader` file.
    installed_only: bool,
    selected: Option<usize>,
    game: Option<Selected>,
    gamedirs: BTreeMap<String, PathBuf>,
    tab: Device,
    /// Keyboard tab: player 1-4.
    kb_player: usize,
    /// Wheel tab: the `input.devices` name (substring of the device name).
    wheel: Option<String>,
    capture: Option<Capture>,
    inputs: Receiver<InputEvent>,
    keyboards: Option<usize>,
    devices: BTreeMap<u32, Connected>,
    status: String,
    running: Option<Running>,
    log: Arc<Mutex<Vec<String>>>,
    show_log: bool,
    /// When Esc last cancelled a capture: that press does not also close the window.
    esc_cancel: Option<Instant>,
    images: Images,
}

impl App {
    fn new(root: PathBuf, inputs: Receiver<InputEvent>, ctx: egui::Context) -> App {
        let mut status = String::new();
        let mut games = Vec::new();
        match config::game_ids(&root) {
            Ok(ids) => {
                for id in ids {
                    let profile = Layers::load(&id, &root).and_then(|l| l.profile(&root));
                    let (name, system) = match &profile {
                        Ok(p) => (p.name.clone().unwrap_or_else(|| id.clone()), p.system.clone()),
                        Err(_) => (id.clone(), id.split('/').next().unwrap_or("").to_string()),
                    };
                    games.push(Game { id, name, system });
                }
            }
            Err(e) => status = format!("{e:#}"),
        }
        games.sort_by_key(|g| g.name.to_lowercase());
        let gamedirs = read_gamedirs(&root).unwrap_or_else(|e| {
            status = format!("{e:#}");
            BTreeMap::new()
        });
        App {
            root,
            games,
            filter: String::new(),
            installed_only: false,
            selected: None,
            game: None,
            gamedirs,
            tab: Device::Gamepad,
            kb_player: 1,
            wheel: None,
            capture: None,
            inputs,
            keyboards: None,
            devices: BTreeMap::new(),
            status,
            running: None,
            log: Arc::new(Mutex::new(Vec::new())),
            show_log: false,
            esc_cancel: None,
            images: Images::new(ctx),
        }
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.capture = None;
        self.wheel = None;
        let id = &self.games[index].id;
        match Layers::load(id, &self.root).and_then(|layers| Ok(Selected { profile: layers.profile(&self.root)?, layers })) {
            Ok(sel) => {
                self.wheel = sel.profile.input.devices.keys().next().cloned();
                self.game = Some(sel);
            }
            Err(e) => {
                self.game = None;
                self.status = format!("{e:#}");
            }
        }
    }

    // ---- mapping tables ----

    /// The edited table's path in the profile.
    fn table_path(&self, device: Device) -> Option<Vec<String>> {
        Some(match device {
            Device::Keyboard => vec!["input".into(), "keyboard".into(), format!("p{}", self.kb_player)],
            Device::Gamepad => vec!["input".into(), "gamepad".into()],
            Device::Wheel => vec!["input".into(), "devices".into(), self.wheel.clone()?, "map".into()],
        })
    }

    /// The table in use (all the layers merged).
    fn table(&self, device: Device) -> MapTable {
        let Some(sel) = &self.game else { return MapTable::new() };
        let input = &sel.profile.input;
        match device {
            Device::Keyboard => input.keyboard.get(&format!("p{}", self.kb_player)).cloned().unwrap_or_default(),
            Device::Gamepad => input.gamepad.clone(),
            Device::Wheel => self.wheel.as_ref().and_then(|w| input.devices.get(w)).map(|d| d.map.clone()).unwrap_or_default(),
        }
    }

    /// Edits a table: the user layer gets what then differs from the system profile.
    fn edit(&mut self, device: Device, f: impl FnOnce(&mut MapTable)) {
        let Some(path) = self.table_path(device) else { return };
        let mut table = self.table(device);
        f(&mut table);
        let Some(sel) = &mut self.game else { return };
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        let diff = bindings::diff(&bindings::table_at(&sel.layers.base, &path), &table);
        bindings::set_at(&mut sel.layers.user, &path, bindings::table_value(&diff));
        if device == Device::Wheel {
            // raw sources even for a device SDL knows as a gamepad
            let raw = (!diff.is_empty()).then_some(Value::Bool(true));
            bindings::set_at(&mut sel.layers.user, &[path[0], path[1], path[2], "raw"], raw);
        }
        self.save_user();
    }

    /// Sets (None: removes) a value of the user layer.
    fn set_user(&mut self, path: &[&str], value: Option<Value>) {
        if let Some(sel) = &mut self.game {
            bindings::set_at(&mut sel.layers.user, path, value);
            self.save_user();
        }
    }

    /// Writes the user profile and reloads the game's profile.
    fn save_user(&mut self) {
        let Some(sel) = &mut self.game else { return };
        let result = (|| -> Result<()> {
            let path = &sel.layers.user_path;
            let empty = sel.layers.user.as_mapping().is_none_or(|m| m.is_empty());
            if empty {
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
            } else {
                std::fs::create_dir_all(path.parent().context("user profile directory")?)?;
                let text = serde_yaml_ng::to_string(&sel.layers.user)?;
                std::fs::write(path, format!("# {} - user overrides (written by the launcher GUI)\n{text}", sel.layers.id))
                    .with_context(|| format!("writing {}", path.display()))?;
            }
            sel.profile = sel.layers.profile(&self.root)?;
            Ok(())
        })();
        self.status = match result {
            Ok(()) => format!("saved {}", sel.layers.user_path.display()),
            Err(e) => format!("{e:#}"),
        };
    }

    // ---- inputs ----

    fn wheel_matches(&self, id: u32) -> bool {
        let (Some(w), Some(d)) = (&self.wheel, self.devices.get(&id)) else { return false };
        d.name.to_ascii_lowercase().contains(&w.to_ascii_lowercase())
    }

    fn start_capture(&mut self, device: Device, row: Row, replace: bool) {
        let rest = self
            .devices
            .iter()
            .flat_map(|(id, d)| d.axes.iter().enumerate().map(move |(i, v)| ((*id, i as u8), *v)))
            .collect();
        self.capture = Some(Capture { device, row, replace, started: Instant::now(), rest });
    }

    fn poll_inputs(&mut self) {
        while let Ok(ev) = self.inputs.try_recv() {
            let caught = self.capture.as_ref().and_then(|c| self.captured(c, &ev));
            match ev {
                InputEvent::Keyboards(n) => self.keyboards = Some(n),
                InputEvent::Added { id, name, is_pad, axes } => {
                    self.devices.insert(id, Connected { name, is_pad, axes });
                }
                InputEvent::Removed(id) => {
                    self.devices.remove(&id);
                }
                InputEvent::JoyAxis { id, index, value } => {
                    if let Some(d) = self.devices.get_mut(&id) {
                        if let Some(slot) = d.axes.get_mut(index as usize) {
                            *slot = value;
                        }
                    }
                }
                InputEvent::Key(code, true) if code == evdev::KeyCode::KEY_ESC.code() && self.capture.is_some() => {
                    self.capture = None;
                    self.status = "cancelled".into();
                    self.esc_cancel = Some(Instant::now());
                    continue;
                }
                _ => {}
            }
            if let Some(c) = caught {
                self.bind(c);
            }
        }
        if self.capture.as_ref().is_some_and(|c| c.started.elapsed() > CAPTURE_TIMEOUT) {
            self.capture = None;
            self.status = "no input pressed: cancelled".into();
        }
    }

    /// The input an event gives to the capture, once pressed far enough.
    fn captured(&self, c: &Capture, ev: &InputEvent) -> Option<Captured> {
        let threshold = if c.row.kind == RowKind::Analog { 8000 } else { 16000 };
        match (c.device, ev) {
            (Device::Keyboard, InputEvent::Key(code, true)) if *code != evdev::KeyCode::KEY_ESC.code() => {
                Some(Captured::Key(*code))
            }
            (Device::Gamepad, InputEvent::PadButton { name, down: true }) => Some(Captured::PadButton(name)),
            (Device::Gamepad, InputEvent::PadAxis { name, value }) if value.abs() > threshold => {
                Some(Captured::PadAxis(name, *value))
            }
            (Device::Wheel, InputEvent::JoyButton { id, index, down: true }) if self.wheel_matches(*id) => {
                Some(Captured::JoyButton(*index))
            }
            (Device::Wheel, InputEvent::JoyHat { id, index, bits }) if *bits != 0 && self.wheel_matches(*id) => {
                Some(Captured::JoyHat(*index, 1 << bits.trailing_zeros()))
            }
            (Device::Wheel, InputEvent::JoyAxis { id, index, value }) if self.wheel_matches(*id) => {
                let rest = c.rest.get(&(*id, *index)).copied().unwrap_or(0);
                ((value - rest).abs() > threshold).then_some(Captured::JoyAxis(*index, *value, rest))
            }
            _ => None,
        }
    }

    fn bind(&mut self, c: Captured) {
        let Some(cap) = &self.capture else { return };
        let Some((src, dst)) = bindings::binding(c, &cap.row) else {
            self.status = format!("{} needs an axis (a stick or the wheel): keep waiting", cap.row.label);
            return;
        };
        let cap = self.capture.take().expect("capture");
        self.edit(cap.device, |t| bindings::assign(t, cap.device, &cap.row, &src, &dst, cap.replace));
        self.status = format!("{}: {}  ({})", cap.row.label, bindings::source_label(cap.device, &src), self.status);
    }

    // ---- dumps and launch ----

    /// The game's dump (folder or image) and whether it can be launched.
    fn dump_state(&mut self, game: usize) -> DumpState {
        let g = &self.games[game];
        let slug = g.slug().to_string();
        let Some(path) = self.gamedirs.get(&g.id).cloned() else { return DumpState::NotSet };
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(squashfs::EXT)) {
            if !path.is_file() {
                return DumpState::Error(format!("{} not found", path.display()));
            }
            return match self.images.get(&path) {
                None => DumpState::Checking,
                Some(Err(e)) => DumpState::Error(e.clone()),
                Some(Ok(files)) => match files.iter().find(|f| f.id.eq_ignore_ascii_case(&slug)) {
                    Some(_) if files.len() > 1 => DumpState::Error(format!(
                        "several .{DUMP_EXT} files in this image ({}): one per image",
                        files.iter().map(|f| f.id.as_str()).collect::<Vec<_>>().join(", ")
                    )),
                    Some(f) => DumpState::Ready { target: path, exe: f.exe.clone() },
                    None if files.is_empty() => DumpState::Error(format!("no .{DUMP_EXT} file at this image's root")),
                    None => DumpState::Error(format!(
                        "this image has no {slug}.{DUMP_EXT} (it has: {})",
                        files.iter().map(|f| format!("{}.{DUMP_EXT}", f.id)).collect::<Vec<_>>().join(", ")
                    )),
                },
            };
        }
        let file = path.join(format!("{slug}.{DUMP_EXT}"));
        if !file.is_file() {
            return DumpState::NoFile;
        }
        match Dump::find(&file.to_string_lossy()) {
            Ok(Some(d)) => DumpState::Ready { target: file, exe: d.exe.display().to_string() },
            Ok(None) => DumpState::NoFile,
            Err(e) => DumpState::Error(format!("{e:#}")),
        }
    }

    fn set_gamedir(&mut self, id: &str, dir: Option<PathBuf>) {
        match dir {
            Some(d) => {
                // read again (the image may have changed)
                self.images.results.remove(&d);
                self.gamedirs.insert(id.to_string(), d)
            }
            None => self.gamedirs.remove(id),
        };
        if let Err(e) = write_gamedirs(&self.root, &self.gamedirs) {
            self.status = format!("{e:#}");
        }
    }

    /// Finds the dumps under a folder: dump folders (their `<gameid>.windowsloader` files) and
    /// SquashFS images (the `.windowsloader` files at their root).
    fn scan(&mut self, dir: &Path) {
        let mut found = Vec::new();
        find_dump_files(dir, 4, &mut found);
        let mut count = 0;
        for file in found {
            let (ids, target) = if file.extension().is_some_and(|e| e.eq_ignore_ascii_case(squashfs::EXT)) {
                let r = squashfs::dump_files(&file).map_err(|e| format!("{e:#}"));
                let ids: Vec<String> = r.as_ref().map(|f| f.iter().map(|f| f.id.to_ascii_lowercase()).collect()).unwrap_or_default();
                self.images.results.insert(file.clone(), Some(r));
                (ids, file.clone())
            } else {
                let Some(parent) = file.parent() else { continue };
                (vec![file.file_stem().unwrap_or_default().to_string_lossy().to_ascii_lowercase()], parent.to_path_buf())
            };
            for stem in ids {
                let games: Vec<String> = self.games.iter().filter(|g| g.slug() == stem).map(|g| g.id.clone()).collect();
                for id in games {
                    self.gamedirs.insert(id, target.clone());
                    count += 1;
                }
            }
        }
        self.status = match write_gamedirs(&self.root, &self.gamedirs) {
            Ok(()) => format!("{count} games found under {}", dir.display()),
            Err(e) => format!("{e:#}"),
        };
    }

    fn launch(&mut self, game: usize) {
        let DumpState::Ready { target: file, .. } = self.dump_state(game) else { return };
        let g = &self.games[game];
        let result = (|| -> Result<Child> {
            let exe = std::env::current_exe()?;
            let mut cmd = Command::new(exe);
            // the game stops with the window, even when the GUI is killed or crashes (the
            // launcher stops wine on SIGTERM)
            unsafe {
                std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }
            let mut child = cmd
                .arg("run")
                .arg(&file)
                .arg("--root")
                .arg(&self.root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("starting the launcher")?;
            let mut log = self.log.lock().expect("log");
            log.clear();
            log.push(format!("$ arcade-launcher run {}", file.display()));
            drop(log);
            for out in [child.stdout.take().map(|o| Box::new(o) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|e| Box::new(e) as _)]
                .into_iter()
                .flatten()
            {
                let log = self.log.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        let mut log = log.lock().expect("log");
                        log.push(line);
                        if log.len() > LOG_LINES {
                            log.remove(0);
                        }
                    }
                });
            }
            Ok(child)
        })();
        match result {
            Ok(child) => {
                self.status = format!("{} started", g.name);
                self.running = Some(Running { child, game: g.name.clone() });
            }
            Err(e) => self.status = format!("{e:#}"),
        }
    }

    fn stop(&mut self) {
        if let Some(r) = &self.running {
            // the launcher stops the game on SIGTERM (wineserver -k)
            let _ = Command::new("kill").args(["-TERM", &r.child.id().to_string()]).status();
            self.status = format!("stopping {}", r.game);
        }
    }

    fn poll_running(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.running else { return };
        match r.child.try_wait() {
            Ok(Some(status)) => {
                self.status = format!("{} exited ({status})", r.game);
                if !status.success() {
                    self.show_log = true;
                }
                self.running = None;
            }
            Ok(None) => ctx.request_repaint_after(Duration::from_millis(500)),
            Err(e) => {
                self.status = format!("{e}");
                self.running = None;
            }
        }
    }

    // ---- UI ----

    fn game_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("search").desired_width(f32::INFINITY));
        });
        let installed: Vec<bool> =
            (0..self.games.len()).map(|i| matches!(self.dump_state(i), DumpState::Ready { .. })).collect();
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.installed_only, "Installed only");
            let count = installed.iter().filter(|i| **i).count();
            ui.weak(format!("{count} / {} installed", self.games.len()));
        });
        ui.separator();
        let filter = self.filter.to_lowercase();
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (i, g) in self.games.iter().enumerate() {
                if !filter.is_empty() && !g.name.to_lowercase().contains(&filter) && !g.id.contains(&filter) {
                    continue;
                }
                let ready = installed[i];
                if self.installed_only && !ready {
                    continue;
                }
                let text = egui::RichText::new(format!("{} {}", if ready { "▶" } else { "  " }, g.name));
                let r = ui.selectable_label(self.selected == Some(i), text).on_hover_text(format!("{} ({})", g.id, g.system));
                if r.clicked() {
                    clicked = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            self.select(i);
        }
    }

    fn game_page(&mut self, ui: &mut egui::Ui, index: usize) {
        let (name, id, system) = {
            let g = &self.games[index];
            (g.name.clone(), g.id.clone(), g.system.clone())
        };
        ui.horizontal(|ui| {
            ui.heading(&name);
            ui.label(egui::RichText::new(format!("{system} · {id}")).weak());
        });
        ui.add_space(4.0);
        self.dump_section(ui, index);
        ui.add_space(8.0);
        if self.game.is_none() {
            ui.colored_label(ui.visuals().error_fg_color, "profile error (see the status bar)");
            return;
        }
        ui.horizontal(|ui| {
            for (d, label) in [(Device::Keyboard, "⌨ Keyboard"), (Device::Gamepad, "🎮 Gamepad"), (Device::Wheel, "🚗 Wheel / joystick")] {
                if ui.selectable_label(self.tab == d, egui::RichText::new(label).size(16.0)).clicked() {
                    self.tab = d;
                    self.capture = None;
                }
            }
        });
        ui.separator();
        match self.tab {
            Device::Keyboard => {
                ui.horizontal(|ui| {
                    ui.label("Player");
                    for p in 1..=wal_protocol::MAX_PLAYERS {
                        if ui.selectable_label(self.kb_player == p, p.to_string()).clicked() {
                            self.kb_player = p;
                            self.capture = None;
                        }
                    }
                    if self.keyboards == Some(0) {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "no keyboard readable in /dev/input (add your user to the input group)",
                        );
                    }
                });
            }
            Device::Gamepad => {
                let pads: Vec<&str> = self.devices.values().filter(|d| d.is_pad).map(|d| d.name.as_str()).collect();
                ui.label(if pads.is_empty() {
                    "No gamepad connected. This mapping applies to every gamepad (players in connection order).".into()
                } else {
                    format!("Every gamepad (players in connection order): {}", pads.join(", "))
                });
            }
            Device::Wheel => self.wheel_header(ui),
        }
        ui.add_space(4.0);
        if self.tab == Device::Wheel && self.wheel.is_none() {
            return;
        }
        self.mapping_table(ui);
    }

    fn dump_section(&mut self, ui: &mut egui::Ui, index: usize) {
        let g = &self.games[index];
        let (id, slug, name) = (g.id.clone(), g.slug().to_string(), g.name.clone());
        let path = self.gamedirs.get(&id).cloned();
        let is_image = path.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(squashfs::EXT)));
        let state = self.dump_state(index);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(if is_image { "Game image:" } else { "Game folder:" });
                match &path {
                    Some(d) => ui.monospace(d.display().to_string()),
                    None => ui.weak("not set"),
                };
            });
            ui.horizontal(|ui| {
                let start = path.as_ref().and_then(|d| d.parent()).map(Path::to_path_buf);
                if ui.button("📂 Choose folder…").clicked() {
                    let mut dialog = rfd::FileDialog::new().set_title(format!("Game folder of {name}"));
                    if let Some(d) = &start {
                        dialog = dialog.set_directory(d);
                    }
                    if let Some(d) = dialog.pick_folder() {
                        self.set_gamedir(&id, Some(d));
                    }
                }
                if ui.button("Choose image…").on_hover_text("the dump packed as a SquashFS image (.squashfs)").clicked() {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(format!("SquashFS image of {name}"))
                        .add_filter("SquashFS image", &[squashfs::EXT]);
                    if let Some(d) = &start {
                        dialog = dialog.set_directory(d);
                    }
                    if let Some(f) = dialog.pick_file() {
                        self.set_gamedir(&id, Some(f));
                    }
                }
                if path.is_some() && ui.button("Clear").clicked() {
                    self.set_gamedir(&id, None);
                }
                if matches!(state, DumpState::NoFile) && ui.button("Choose the game executable…").clicked() {
                    self.pick_exe(&id, &slug);
                }
                ui.separator();
                let ready = matches!(state, DumpState::Ready { .. });
                match &self.running {
                    Some(r) => {
                        ui.label(format!("▶ {} running", r.game));
                        if ui.button("⏹ Stop").clicked() {
                            self.stop();
                        }
                    }
                    None => {
                        let launch = ui.add_enabled(ready, egui::Button::new(egui::RichText::new("▶ Launch").strong().size(16.0)));
                        if launch.clicked() {
                            self.launch(index);
                        }
                    }
                }
            });
            match &state {
                DumpState::NotSet => ui.weak(format!(
                    "The dump folder holding {slug}.{DUMP_EXT}, or its SquashFS image (Scan finds them in a folder)."
                )),
                DumpState::NoFile => ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("No {slug}.{DUMP_EXT} in this folder: choose the game executable to create it."),
                ),
                DumpState::Checking => ui.weak("reading the image…"),
                DumpState::Ready { exe, .. } if is_image => {
                    ui.weak(format!("✔ {slug}.{DUMP_EXT} in the image: {exe} (mounted with an overlay for its writes)"))
                }
                DumpState::Ready { exe, .. } => ui.weak(format!("✔ {exe}")),
                DumpState::Error(e) => ui.colored_label(ui.visuals().error_fg_color, e),
            };
        });
    }

    /// Writes `<slug>.windowsloader` (the executable's path in the dump).
    fn pick_exe(&mut self, id: &str, slug: &str) {
        let Some(dir) = self.gamedirs.get(id).cloned() else { return };
        let Some(exe) = rfd::FileDialog::new().set_directory(&dir).add_filter("Windows executable", &["exe", "EXE"]).pick_file() else {
            return;
        };
        let Ok(rel) = exe.strip_prefix(&dir) else {
            self.status = format!("{} is not in the game folder {}", exe.display(), dir.display());
            return;
        };
        let file = dir.join(format!("{slug}.{DUMP_EXT}"));
        let rel = rel.to_string_lossy().replace('\\', "/");
        self.status = match std::fs::write(&file, format!("{rel}\n")) {
            Ok(()) => format!("wrote {}", file.display()),
            Err(e) => format!("writing {}: {e}", file.display()),
        };
    }

    fn wheel_header(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = &self.game else { return };
        let mut names: Vec<String> = sel.profile.input.devices.keys().cloned().collect();
        for d in self.devices.values() {
            if !names.iter().any(|n| d.name.to_ascii_lowercase().contains(&n.to_ascii_lowercase())) {
                names.push(d.name.clone());
            }
        }
        let player = self.wheel.as_ref().and_then(|w| sel.profile.input.devices.get(w)).and_then(|d| d.player);
        ui.label("Wheels, pedals and other joysticks are used only when mapped here (by device name).");
        let mut chosen = None;
        let mut new_player = None;
        ui.horizontal(|ui| {
            ui.label("Device");
            egui::ComboBox::from_id_salt("wheel-device")
                .width(320.0)
                .selected_text(self.wheel.clone().unwrap_or_else(|| "choose a device".into()))
                .show_ui(ui, |ui| {
                    for n in &names {
                        let connected = self.devices.values().any(|d| d.name.to_ascii_lowercase().contains(&n.to_ascii_lowercase()));
                        let text = if connected { format!("🔌 {n}") } else { n.clone() };
                        if ui.selectable_label(self.wheel.as_ref() == Some(n), text).clicked() {
                            chosen = Some(n.clone());
                        }
                    }
                });
            if self.wheel.is_some() {
                ui.label("Player");
                egui::ComboBox::from_id_salt("wheel-player")
                    .selected_text(player.map_or("auto".to_string(), |p| p.to_string()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(player.is_none(), "auto").clicked() {
                            new_player = Some(None);
                        }
                        for p in 1..=wal_protocol::MAX_PLAYERS {
                            if ui.selectable_label(player == Some(p), p.to_string()).clicked() {
                                new_player = Some(Some(p));
                            }
                        }
                    });
            }
        });
        if let Some(n) = chosen {
            self.wheel = Some(n);
            self.capture = None;
        }
        if let (Some(p), Some(w)) = (new_player, self.wheel.clone()) {
            self.set_user(&["input", "devices", &w, "player"], p.map(|p| Value::Number((p as u64).into())));
        }
        // live axes of the device, to see which one moves
        let live: Vec<(String, Vec<i32>)> = self
            .devices
            .iter()
            .filter(|(id, _)| self.wheel_matches(**id))
            .map(|(_, d)| (d.name.clone(), d.axes.clone()))
            .collect();
        for (_, axes) in live {
            ui.horizontal_wrapped(|ui| {
                for (i, v) in axes.iter().enumerate() {
                    ui.label(format!("Axis {i}"));
                    ui.add(egui::ProgressBar::new((*v as f32 + 32768.0) / 65535.0).desired_width(70.0));
                }
            });
        }
        if self.wheel.is_some() && live_is_empty(&self.devices, self.wheel.as_deref()) {
            ui.weak("(not connected)");
        }
    }

    fn mapping_table(&mut self, ui: &mut egui::Ui) {
        let device = self.tab;
        let Some(sel) = &self.game else { return };
        let rows = bindings::rows(&sel.profile, device);
        let table = self.table(device);
        // gamepad: the left stick on the d-pad's directions (as the launcher adds it)
        let auto: MapTable = match device {
            Device::Gamepad => crate::mapping::stick_dpad(&table, |t| sel.profile.uses(t)).into_iter().collect(),
            _ => MapTable::new(),
        };
        let mut action: Option<Action> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("mapping").num_columns(3).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                for row in &rows {
                    ui.label(egui::RichText::new(&row.label).strong());
                    ui.horizontal_wrapped(|ui| {
                        let bound = bindings::bound(&table, device, row);
                        let automatic = bindings::bound(&auto, device, row);
                        if bound.is_empty() && automatic.is_empty() {
                            ui.weak("—");
                        }
                        for (src, _) in automatic {
                            let text = format!("{} (auto)", bindings::source_label(device, &src));
                            ui.add_enabled(
                                false,
                                egui::Button::new(egui::RichText::new(text).size(15.0))
                                    .wrap_mode(egui::TextWrapMode::Extend)
                                    .min_size(egui::vec2(150.0, 28.0)),
                            )
                            .on_disabled_hover_text(
                                "automatic: the left stick follows the d-pad while it drives nothing the game uses \
                                 (map the stick to something else to free it)",
                            );
                        }
                        for (src, inverted) in bound {
                            let mut text = bindings::source_label(device, &src);
                            if inverted && row.kind == RowKind::Analog {
                                text += " (inverted)";
                            }
                            let chip = ui
                                .add(
                                    egui::Button::new(egui::RichText::new(format!("{text}   ✖")).size(15.0))
                                        .wrap_mode(egui::TextWrapMode::Extend)
                                        .min_size(egui::vec2(150.0, 28.0)),
                                )
                                .on_hover_text(format!("{src}: remove"));
                            if chip.clicked() {
                                action = Some(Action::Remove(src));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let waiting = self.capture.as_ref().is_some_and(|c| c.row == *row && c.device == device);
                        if waiting {
                            let what = match (device, row.kind) {
                                (Device::Keyboard, _) => "press a key",
                                (_, RowKind::Analog) => "move the axis (right / down)",
                                (_, RowKind::Pedal) => "press the pedal / trigger",
                                _ => "press a button",
                            };
                            ui.colored_label(ui.visuals().warn_fg_color, format!("{what}… (Esc cancels)"));
                            if ui.small_button("Cancel").clicked() {
                                action = Some(Action::Cancel);
                            }
                        } else {
                            if ui.button("Replace…").on_hover_text("remove this control's inputs, then bind the one you press").clicked() {
                                action = Some(Action::Capture(row.clone(), true));
                            }
                            if ui.button("Add…").on_hover_text("keep this control's inputs and bind one more").clicked() {
                                action = Some(Action::Capture(row.clone(), false));
                            }
                        }
                    });
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Reset to the game's defaults").clicked() {
                    action = Some(Action::Reset);
                }
                if device == Device::Wheel && ui.button("Forget this device").clicked() {
                    action = Some(Action::Forget);
                }
            });
        });
        match action {
            Some(Action::Remove(src)) => self.edit(device, |t| {
                t.remove(&src);
            }),
            Some(Action::Capture(row, replace)) => self.start_capture(device, row, replace),
            Some(Action::Cancel) => self.capture = None,
            Some(Action::Reset) => {
                if let Some(path) = self.table_path(device) {
                    let path: Vec<&str> = path.iter().map(String::as_str).collect();
                    self.set_user(&path, None);
                }
            }
            Some(Action::Forget) => {
                if let Some(w) = self.wheel.take() {
                    self.set_user(&["input", "devices", &w], None);
                    self.wheel = self.game.as_ref().and_then(|s| s.profile.input.devices.keys().next().cloned());
                }
            }
            None => {}
        }
    }
}

/// The window closed: the running game stops with it.
impl Drop for App {
    fn drop(&mut self) {
        let Some(r) = &mut self.running else { return };
        eprintln!("gui: stopping {}", r.game);
        let _ = Command::new("kill").args(["-TERM", &r.child.id().to_string()]).status();
        // the launcher kills wine (wineserver -k) and exits
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !matches!(r.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = r.child.kill();
    }
}

enum Action {
    Remove(String),
    Capture(Row, bool),
    Cancel,
    Reset,
    Forget,
}

fn live_is_empty(devices: &BTreeMap<u32, Connected>, wheel: Option<&str>) -> bool {
    let Some(w) = wheel.map(str::to_ascii_lowercase) else { return true };
    !devices.values().any(|d| d.name.to_ascii_lowercase().contains(&w))
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_inputs();
        self.poll_running(ctx);
        if self.capture.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Windows Arcade Loader");
                ui.separator();
                if ui.button("🔎 Scan a folder of dumps…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Folder of game dumps").pick_folder() {
                        self.scan(&dir);
                    }
                }
                ui.toggle_value(&mut self.show_log, "📄 Game log");
            });
        });
        // Esc (window focused, no game running, no input awaited) quits, as Exit
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape))
            && self.capture.is_none()
            && self.running.is_none()
            && self.esc_cancel.is_none_or(|t| t.elapsed() > Duration::from_millis(500));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let exit = ui.button(egui::RichText::new("Exit").size(16.0)).on_hover_text("quit the launcher (Esc); a running game stops");
                    if exit.clicked() || esc {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });
        if self.show_log {
            egui::Panel::bottom("log").resizable(true).default_size(200.0).show(ui, |ui| {
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                    for line in self.log.lock().expect("log").iter() {
                        ui.monospace(line);
                    }
                });
            });
        }
        egui::Panel::left("games").resizable(true).default_size(320.0).show(ui, |ui| self.game_list(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.selected {
            Some(i) => self.game_page(ui, i),
            None => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.heading(format!("{} games", self.games.len()));
                    ui.label("Choose a game on the left to map its controls and set its dump folder.");
                });
            }
        });
    }
}

fn read_gamedirs(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let path = root.join(USER_PROFILES).join(GAMEDIRS);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let dirs: Option<BTreeMap<String, PathBuf>> =
        serde_yaml_ng::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(dirs.unwrap_or_default())
}

fn write_gamedirs(root: &Path, dirs: &BTreeMap<String, PathBuf>) -> Result<()> {
    let path = root.join(USER_PROFILES).join(GAMEDIRS);
    std::fs::create_dir_all(root.join(USER_PROFILES))?;
    let text = serde_yaml_ng::to_string(dirs)?;
    std::fs::write(&path, format!("# Game dump folders (launcher GUI): <system>/<game>: <folder>\n{text}"))
        .with_context(|| format!("writing {}", path.display()))
}

fn find_dump_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        let ext = |x: &str| path.extension().is_some_and(|e| e.eq_ignore_ascii_case(x));
        if kind.is_file() && (ext(DUMP_EXT) || ext(squashfs::EXT)) {
            out.push(path);
        } else if kind.is_dir() && depth > 0 {
            find_dump_files(&path, depth - 1, out);
        }
    }
}
