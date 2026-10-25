//! Frontend window (`arcade-launcher` without a game): every supported game, its controls
//! mapped per device (keyboard, gamepad, wheel, lightgun) into its user profile, its dump folder, and
//! its launch.

mod bindings;
mod calibration;
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

use crate::config::{self, DUMP_EXT, Dump, GameOption, Layers, MapTable, OptionKind, Profile, USER_PROFILES};
use crate::squashfs::{self, DumpFile};
use crate::mapping::RawKey;
use crate::eamuse_card;
use crate::nesica_card::{self, Card};
use crate::wheels::{self, Calibration, Wheels};
use bindings::{Captured, Device, Row, RowKind};
use calibration::{Step, Wizard};
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

/// The game page's tab: one of the input mapping tables, or the game's own options.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tab {
    Keyboard,
    Gamepad,
    Wheel,
    Gun,
    Joystick,
    Options,
}

impl Tab {
    /// The tab's mapping table (Options has none).
    fn device(self) -> Option<Device> {
        match self {
            Tab::Keyboard => Some(Device::Keyboard),
            Tab::Gamepad => Some(Device::Gamepad),
            Tab::Wheel => Some(Device::Wheel),
            Tab::Gun => Some(Device::Gun),
            Tab::Joystick => Some(Device::Joystick),
            Tab::Options => None,
        }
    }
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
    /// Gun / mouse positions when the capture started.
    gun_rest: Vec<[i32; 2]>,
}

/// A lightgun or a mouse aiming like one (player order).
struct Pointer {
    name: String,
    is_gun: bool,
    pos: [i32; 2],
    offscreen: bool,
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
    tab: Tab,
    /// The Options tab's text fields' drafts, until Enter or the field loses the focus.
    option_buffers: BTreeMap<String, String>,
    /// Keyboard tab: player 1-4.
    kb_player: usize,
    /// Wheel tab: the `input.devices` name (substring of the device name).
    joystick: Option<String>,
    capture: Option<Capture>,
    inputs: Receiver<InputEvent>,
    keyboards: Option<usize>,
    devices: BTreeMap<u32, Connected>,
    /// Lightguns and mice (None: not read yet).
    pointers: Option<Vec<Pointer>>,
    status: String,
    running: Option<Running>,
    log: Arc<Mutex<Vec<String>>>,
    show_log: bool,
    /// When Esc last cancelled a capture: that press does not also close the window.
    esc_cancel: Option<Instant>,
    images: Images,
    /// Wheel calibrations (`wheels.rs`), the calibration window and its wizard.
    calibrations: Wheels,
    show_wheels: bool,
    wizard: Option<Wizard>,
    /// The last gamepad button a calibrated wheel pressed (the window's test).
    wheel_pressed: Option<&'static str>,
    /// The NESiCA card window (`nesica_card.rs`): the card edited, and as saved.
    show_card: bool,
    card: Card,
    card_saved: Option<Card>,
    /// The e-Amusement card window (`eamuse_card.rs`): the card edited, and as saved.
    show_eamuse: bool,
    eamuse: eamuse_card::Card,
    eamuse_saved: Option<eamuse_card::Card>,
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
        let calibrations = wheels::load(&root).unwrap_or_else(|e| {
            status = format!("{e:#}");
            Wheels::new()
        });
        let card_saved = nesica_card::load(&root).unwrap_or_else(|e| {
            status = format!("{e:#}");
            None
        });
        let card = card_saved.clone().unwrap_or_else(|| Card { card_id: nesica_card::DEFAULT_ID.into(), player_name: String::new() });
        let eamuse_saved = eamuse_card::load(&root).unwrap_or_else(|e| {
            status = format!("{e:#}");
            None
        });
        let eamuse = eamuse_saved.clone().unwrap_or_else(|| eamuse_card::Card { card_id: eamuse_card::DEFAULT_ID.into(), ..Default::default() });
        App {
            root,
            games,
            filter: String::new(),
            installed_only: false,
            selected: None,
            game: None,
            gamedirs,
            tab: Tab::Gamepad,
            option_buffers: BTreeMap::new(),
            kb_player: 1,
            joystick: None,
            capture: None,
            inputs,
            keyboards: None,
            devices: BTreeMap::new(),
            pointers: None,
            status,
            running: None,
            log: Arc::new(Mutex::new(Vec::new())),
            show_log: false,
            esc_cancel: None,
            images: Images::new(ctx),
            calibrations,
            show_wheels: false,
            wizard: None,
            wheel_pressed: None,
            show_card: false,
            card,
            card_saved,
            show_eamuse: false,
            eamuse,
            eamuse_saved,
        }
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.capture = None;
        self.joystick = None;
        // the previous game's text drafts are its (the profile still loaded), then forgotten
        self.commit_option_buffers();
        self.option_buffers.clear();
        let id = &self.games[index].id;
        match Layers::load(id, &self.root).and_then(|layers| Ok(Selected { profile: layers.profile(&self.root)?, layers })) {
            Ok(sel) => {
                self.joystick = sel.profile.input.devices.keys().next().cloned();
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
            Device::Wheel => vec!["input".into(), "wheel".into()],
            Device::Joystick => vec!["input".into(), "devices".into(), self.joystick.clone()?, "map".into()],
            Device::Gun => vec!["input".into(), "gun".into()],
        })
    }

    /// The table in use (all the layers merged).
    fn table(&self, device: Device) -> MapTable {
        let Some(sel) = &self.game else { return MapTable::new() };
        let input = &sel.profile.input;
        match device {
            Device::Keyboard => input.keyboard.get(&format!("p{}", self.kb_player)).cloned().unwrap_or_default(),
            Device::Gamepad => input.gamepad.clone(),
            Device::Wheel => crate::mapping::wheel_table(&input.gamepad, &input.wheel),
            Device::Joystick => self.joystick.as_ref().and_then(|w| input.devices.get(w)).map(|d| d.map.clone()).unwrap_or_default(),
            Device::Gun => input.gun.clone(),
        }
    }

    /// Edits a table: the user layer gets what then differs from the system profile.
    fn edit(&mut self, device: Device, f: impl FnOnce(&mut MapTable)) {
        let Some(path) = self.table_path(device) else { return };
        let mut table = self.table(device);
        f(&mut table);
        let Some(sel) = &mut self.game else { return };
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        let mut base = bindings::table_at(&sel.layers.base, &path);
        if device == Device::Wheel {
            // the wheel's defaults: the gamepad mapping in use
            base = crate::mapping::wheel_table(&sel.profile.input.gamepad, &base);
        }
        let diff = bindings::diff(&base, &table);
        bindings::set_at(&mut sel.layers.user, &path, bindings::table_value(&diff));
        if device == Device::Joystick {
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

    fn joystick_matches(&self, id: u32) -> bool {
        let (Some(w), Some(d)) = (&self.joystick, self.devices.get(&id)) else { return false };
        d.name.to_ascii_lowercase().contains(&w.to_ascii_lowercase())
    }

    fn start_capture(&mut self, device: Device, row: Row, replace: bool) {
        let rest = self
            .devices
            .iter()
            .flat_map(|(id, d)| d.axes.iter().enumerate().map(move |(i, v)| ((*id, i as u8), *v)))
            .collect();
        let gun_rest = self.pointers.iter().flatten().map(|p| p.pos).collect();
        self.capture = Some(Capture { device, row, replace, started: Instant::now(), rest, gun_rest });
    }

    /// The calibration of a connected device.
    fn calibration(&self, id: u32) -> Option<&Calibration> {
        wheels::find(&self.calibrations, &self.devices.get(&id)?.name)
    }

    /// A calibrated wheel's inputs as a gamepad's (its own SDL gamepad events dropped): None
    /// for the other devices' events.
    fn as_pad(&mut self, ev: &InputEvent) -> Option<Vec<InputEvent>> {
        let (id, raw, value) = match *ev {
            InputEvent::PadButton { id, .. } | InputEvent::PadAxis { id, .. } => {
                return self.calibration(id).map(|_| Vec::new());
            }
            InputEvent::JoyButton { id, index, down } => (id, RawKey::JoyButton(index), down as i32),
            InputEvent::JoyAxis { id, index, value } => (id, RawKey::JoyAxis(index), value),
            InputEvent::JoyHat { id, index, bits } => (id, RawKey::JoyHat(index), bits as i32),
            _ => return None,
        };
        let named = self.calibration(id)?.named(raw, value);
        let mut out = Vec::new();
        for (name, value) in named {
            if matches!(raw, RawKey::JoyAxis(_)) {
                out.push(InputEvent::PadAxis { id, name, value });
            } else {
                if value != 0 {
                    self.wheel_pressed = Some(name);
                }
                out.push(InputEvent::PadButton { id, name, down: value != 0 });
            }
        }
        Some(out)
    }

    /// The calibration wizard's device: its inputs go to the wizard (true: taken).
    fn wizard_input(&mut self, ev: &InputEvent) -> bool {
        let Some(w) = &mut self.wizard else { return false };
        match *ev {
            InputEvent::JoyAxis { id, index, value } if id == w.id => w.axis_moved(index, value),
            InputEvent::JoyButton { id, index, down: true } if id == w.id => w.pressed(format!("button{index}")),
            InputEvent::JoyHat { id, index, bits } if id == w.id && bits != 0 => w.pressed(calibration::hat_source(index, bits)),
            InputEvent::JoyButton { id, .. } | InputEvent::JoyHat { id, .. } if id == w.id => {}
            InputEvent::PadButton { id, .. } | InputEvent::PadAxis { id, .. } if id == w.id => {}
            _ => return false,
        }
        true
    }

    fn poll_inputs(&mut self) {
        while let Ok(ev) = self.inputs.try_recv() {
            let taken = self.wizard_input(&ev);
            let mut caught = None;
            if !taken {
                caught = self.capture.as_ref().and_then(|c| self.captured(c, &ev));
                if let Some(pad) = self.as_pad(&ev) {
                    caught = caught.or_else(|| pad.iter().find_map(|p| self.capture.as_ref().and_then(|c| self.captured(c, p))));
                }
            }
            match ev {
                InputEvent::Keyboards(n) => self.keyboards = Some(n),
                InputEvent::Added { id, name, is_pad, axes } => {
                    self.devices.insert(id, Connected { name, is_pad, axes });
                }
                InputEvent::Removed(id) => {
                    self.devices.remove(&id);
                }
                InputEvent::Pointers(ref found) => {
                    self.pointers = Some(
                        found
                            .iter()
                            .map(|(name, is_gun)| Pointer { name: name.clone(), is_gun: *is_gun, pos: [0, 0], offscreen: false })
                            .collect(),
                    );
                }
                InputEvent::GunAxis { index, axis, value } => {
                    if let Some(p) = self.pointers.as_mut().and_then(|p| p.get_mut(index)) {
                        p.pos[axis as usize & 1] = value;
                    }
                }
                InputEvent::GunOffscreen { index, offscreen } => {
                    if let Some(p) = self.pointers.as_mut().and_then(|p| p.get_mut(index)) {
                        p.offscreen = offscreen;
                    }
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
            // gamepads in the Gamepad tab, calibrated wheels (their inputs as a gamepad's) in the Wheel tab
            (Device::Gamepad | Device::Wheel, InputEvent::PadButton { id, name, down: true })
                if (c.device == Device::Wheel) == self.calibration(*id).is_some() =>
            {
                Some(Captured::PadButton(name))
            }
            (Device::Gamepad | Device::Wheel, InputEvent::PadAxis { id, name, value })
                if value.abs() > threshold && (c.device == Device::Wheel) == self.calibration(*id).is_some() =>
            {
                Some(Captured::PadAxis(name, *value))
            }
            (Device::Joystick, InputEvent::JoyButton { id, index, down: true }) if self.joystick_matches(*id) => {
                Some(Captured::JoyButton(*index))
            }
            (Device::Joystick, InputEvent::JoyHat { id, index, bits }) if *bits != 0 && self.joystick_matches(*id) => {
                Some(Captured::JoyHat(*index, 1 << bits.trailing_zeros()))
            }
            (Device::Joystick, InputEvent::JoyAxis { id, index, value }) if self.joystick_matches(*id) => {
                let rest = c.rest.get(&(*id, *index)).copied().unwrap_or(0);
                ((value - rest).abs() > threshold).then_some(Captured::JoyAxis(*index, *value, rest))
            }
            (Device::Gun, InputEvent::GunKey { code, down: true }) => Some(Captured::Key(*code)),
            (Device::Gun, InputEvent::GunOffscreen { offscreen: true, .. }) if c.row.kind != RowKind::Analog => {
                Some(Captured::GunOffscreen)
            }
            // aiming: the axis moved far from where it was
            (Device::Gun, InputEvent::GunAxis { index, axis, value }) if c.row.kind == RowKind::Analog => {
                let rest = c.gun_rest.get(*index).map_or(0, |p| p[*axis as usize & 1]);
                ((value - rest).abs() > threshold).then_some(Captured::GunAxis(*axis))
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

    // ---- game options ----

    /// The option's value in use (the merged profile's environment, empty when unset).
    fn option_value(&self, key: &str) -> String {
        self.game
            .as_ref()
            .and_then(|sel| sel.profile.env.get(key))
            .cloned()
            .unwrap_or_default()
    }

    /// The option's value in the game's default (without the user layer).
    fn option_base(&self, key: &str) -> Option<String> {
        self.game
            .as_ref()?
            .layers
            .base
            .get("env")
            .and_then(|e| e.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The option differs from the game's default (the user layer sets it).
    fn option_modified(&self, key: &str) -> bool {
        self.game
            .as_ref()
            .and_then(|sel| sel.layers.user.get("env"))
            .is_some_and(|e| e.get(key).is_some())
    }

    /// Sets (None: removes) the option in the user layer, when it differs from the game's
    /// default.
    fn set_option(&mut self, key: &str, value: Option<String>) {
        let store = value.filter(|v| self.option_base(key).as_deref() != Some(v.as_str()));
        self.set_user(&["env", key], store.map(Value::String));
    }

    /// A text field's draft, committed (saved when it differs from the value in use).
    fn commit_option_buffer(&mut self, key: &str) {
        let Some(buf) = self.option_buffers.get(key).cloned() else { return };
        let buf = buf.trim();
        if buf == self.option_value(key) {
            return;
        }
        self.set_option(key, Some(buf.to_string()));
        self.option_buffers.insert(key.to_string(), buf.to_string());
    }

    /// Every text field's draft committed (leaving the Options tab or the game).
    fn commit_option_buffers(&mut self) {
        for key in self.option_buffers.keys().cloned().collect::<Vec<_>>() {
            self.commit_option_buffer(&key);
        }
    }

    /// The option back to the game's default (removed from the user layer).
    fn reset_option(&mut self, key: &str) {
        self.option_buffers.remove(key);
        self.set_option(key, None);
    }

    /// Every declared option back to the game's default.
    fn reset_all_options(&mut self) {
        let Some(sel) = &mut self.game else { return };
        let keys: Vec<String> =
            sel.profile.options.iter().flat_map(|g| g.options.iter()).map(|o| o.key.clone()).collect();
        if let Some(env) = sel.layers.user.get_mut("env").and_then(Value::as_mapping_mut) {
            for key in &keys {
                env.remove(&Value::String(key.clone()));
            }
        }
        self.option_buffers.clear();
        self.save_user();
    }

    /// The game's own options (the system profile's `options`), its environment variables.
    fn options_page(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = &self.game else { return };
        let options = sel.profile.options.clone();
        ui.label(
            "The game's own options (its environment variables). Changes are saved to your profile \
             and apply from the next launch.",
        );
        let mut reset_all = false;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut first = true;
            for group in &options {
                if group.options.is_empty() {
                    continue;
                }
                if !first {
                    ui.add_space(8.0);
                }
                first = false;
                if let Some(title) = &group.title {
                    ui.strong(title);
                }
                egui::Grid::new("game-options").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    for opt in &group.options {
                        let label = ui.label(opt.label.as_deref().unwrap_or(&opt.key));
                        label.on_hover_text(option_hover(opt));
                        ui.horizontal(|ui| {
                            self.option_control(ui, opt);
                            if self.option_modified(&opt.key) {
                                ui.weak("·");
                                if ui.small_button("↺").on_hover_text("reset to the game's default").clicked() {
                                    self.reset_option(&opt.key);
                                }
                            }
                        });
                        ui.end_row();
                    }
                });
            }
            ui.add_space(10.0);
            if ui.button("Reset all to the game's defaults").clicked() {
                reset_all = true;
            }
        });
        if reset_all {
            self.reset_all_options();
        }
    }

    /// One option's control (its value: the merged profile's environment variable).
    fn option_control(&mut self, ui: &mut egui::Ui, opt: &GameOption) {
        let current = self.option_value(&opt.key);
        match opt.kind {
            OptionKind::Bool => {
                let mut on = matches!(current.as_str(), "1" | "true" | "on");
                if ui.checkbox(&mut on, "").changed() {
                    self.set_option(&opt.key, Some(if on { "1".into() } else { "0".into() }));
                }
            }
            OptionKind::Number => {
                let (min, max) = (opt.min.unwrap_or(0), opt.max.unwrap_or(i32::MAX));
                let mut value: i32 = current.parse().unwrap_or(min).clamp(min, max);
                let changed = if let (Some(min), Some(max)) = (opt.min, opt.max) {
                    ui.add_sized([180.0, 0.0], egui::Slider::new(&mut value, min..=max)).changed()
                } else {
                    ui.add(egui::DragValue::new(&mut value)).changed()
                };
                if changed {
                    self.set_option(&opt.key, Some(value.to_string()));
                }
            }
            OptionKind::Choice => {
                let selected = if opt.choices.contains(&current) { current.clone() } else { "…".to_string() };
                egui::ComboBox::from_id_salt(format!("option-{}", opt.key))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for c in &opt.choices {
                            if ui.selectable_label(current == *c, c).clicked() {
                                self.set_option(&opt.key, Some(c.clone()));
                            }
                        }
                    });
            }
            OptionKind::Text => {
                let buf = self.option_buffers.entry(opt.key.clone()).or_insert_with(|| current.clone());
                let edit = ui
                    .add(egui::TextEdit::singleline(buf).desired_width(240.0).font(egui::TextStyle::Monospace))
                    .on_hover_text(opt.key.clone());
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if edit.lost_focus() || (enter && edit.has_focus()) {
                    self.commit_option_buffer(&opt.key);
                }
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
        let has_options = self
            .game
            .as_ref()
            .is_some_and(|sel| sel.profile.options.iter().any(|g| !g.options.is_empty()));
        ui.horizontal(|ui| {
            let tabs = [
                (Tab::Keyboard, "⌨ Keyboard"),
                (Tab::Gamepad, "🎮 Gamepad"),
                (Tab::Wheel, "🚗 Wheel"),
                (Tab::Gun, "🔫 Lightgun"),
                (Tab::Joystick, "🕹 Other joystick"),
            ];
            for (t, label) in tabs {
                if ui.selectable_label(self.tab == t, egui::RichText::new(label).size(16.0)).clicked() {
                    self.commit_option_buffers();
                    self.tab = t;
                    self.capture = None;
                }
            }
            if has_options
                && ui
                    .selectable_label(self.tab == Tab::Options, egui::RichText::new("⚙ Options").size(16.0))
                    .on_hover_text("the game's own options")
                    .clicked()
            {
                self.commit_option_buffers();
                self.tab = Tab::Options;
                self.capture = None;
            }
        });
        ui.separator();
        match self.tab {
            Tab::Keyboard => {
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
            Tab::Gamepad => {
                let pads: Vec<&str> = self
                    .devices
                    .values()
                    .filter(|d| d.is_pad && wheels::find(&self.calibrations, &d.name).is_none())
                    .map(|d| d.name.as_str())
                    .collect();
                ui.label(if pads.is_empty() {
                    "No gamepad connected. This mapping applies to every gamepad (players in connection order).".into()
                } else {
                    format!("Every gamepad (players in connection order): {}", pads.join(", "))
                });
            }
            Tab::Wheel => {
                let wheels: Vec<&str> = self
                    .devices
                    .values()
                    .filter(|d| wheels::find(&self.calibrations, &d.name).is_some())
                    .map(|d| d.name.as_str())
                    .collect();
                ui.label(
                    "Every calibrated wheel (🚗 Wheel calibration). Its defaults are this game's gamepad mapping \
                     (wheel = left stick X, gas = RT, brake = LT, clutch = right stick down); changes here are for the wheel only.",
                );
                if wheels.is_empty() {
                    ui.weak("No calibrated wheel connected.");
                } else {
                    ui.label(format!("Connected: {}", wheels.join(", ")));
                }
            }
            Tab::Joystick => self.joystick_header(ui),
            Tab::Gun => self.gun_header(ui),
            Tab::Options => {}
        }
        if self.tab == Tab::Options {
            self.options_page(ui);
            return;
        }
        ui.add_space(4.0);
        if self.tab == Tab::Joystick && self.joystick.is_none() {
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

    fn gun_header(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = &self.game else { return };
        let enabled = sel.profile.input.guns_enabled;
        let base = sel.layers.base.get("input").and_then(|i| i.get("guns_enabled")).and_then(Value::as_bool).unwrap_or(false);
        ui.label(
            "Every lightgun, then every mouse aiming like one (players in device order: guns first). \
             Aim = the gun position (x / y), pointing off screen = a gun aiming outside the screen.",
        );
        let mut on = enabled;
        ui.checkbox(&mut on, "Lightguns and mice read in this game");
        if on != enabled {
            // the user layer only when it differs from the game's default
            self.set_user(&["input", "guns_enabled"], (on != base).then_some(Value::Bool(on)));
        }
        match &self.pointers {
            None => {
                ui.weak("looking for lightguns and mice…");
            }
            Some(p) if p.is_empty() => {
                ui.weak("No lightgun or mouse readable in /dev/input (add your user to the input group).");
            }
            Some(pointers) => {
                for (i, p) in pointers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("P{} {} {}", i + 1, if p.is_gun { "🔫" } else { "🖱" }, p.name));
                        for (axis, v) in ["x", "y"].iter().zip(p.pos) {
                            ui.add(egui::ProgressBar::new((v as f32 + 32768.0) / 65535.0).desired_width(70.0).text(*axis));
                        }
                        if p.offscreen {
                            ui.weak("off screen");
                        }
                    });
                }
            }
        }
    }

    fn joystick_header(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = &self.game else { return };
        let mut names: Vec<String> = sel.profile.input.devices.keys().cloned().collect();
        for d in self.devices.values() {
            if !names.iter().any(|n| d.name.to_ascii_lowercase().contains(&n.to_ascii_lowercase())) {
                names.push(d.name.clone());
            }
        }
        let player = self.joystick.as_ref().and_then(|w| sel.profile.input.devices.get(w)).and_then(|d| d.player);
        ui.label(
            "Wheels, pedals or other joysticks not calibrated (🚗 Wheel calibration), mapped here by device name \
             with raw inputs, for this game only (a calibrated wheel mapped here loses its calibration in this game).",
        );
        let mut chosen = None;
        let mut new_player = None;
        ui.horizontal(|ui| {
            ui.label("Device");
            egui::ComboBox::from_id_salt("wheel-device")
                .width(320.0)
                .selected_text(self.joystick.clone().unwrap_or_else(|| "choose a device".into()))
                .show_ui(ui, |ui| {
                    for n in &names {
                        let connected = self.devices.values().any(|d| d.name.to_ascii_lowercase().contains(&n.to_ascii_lowercase()));
                        let text = if connected { format!("🔌 {n}") } else { n.clone() };
                        if ui.selectable_label(self.joystick.as_ref() == Some(n), text).clicked() {
                            chosen = Some(n.clone());
                        }
                    }
                });
            if self.joystick.is_some() {
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
            self.joystick = Some(n);
            self.capture = None;
        }
        if let (Some(p), Some(w)) = (new_player, self.joystick.clone()) {
            self.set_user(&["input", "devices", &w, "player"], p.map(|p| Value::Number((p as u64).into())));
        }
        // live axes of the device, to see which one moves
        let live: Vec<(String, Vec<i32>)> = self
            .devices
            .iter()
            .filter(|(id, _)| self.joystick_matches(**id))
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
        if self.joystick.is_some() && live_is_empty(&self.devices, self.joystick.as_deref()) {
            ui.weak("(not connected)");
        }
    }

    fn mapping_table(&mut self, ui: &mut egui::Ui) {
        let Some(device) = self.tab.device() else { return };
        let Some(sel) = &self.game else { return };
        let rows = bindings::rows(&sel.profile, device);
        let table = self.table(device);
        // gamepad: the left stick on the d-pad's directions (as the launcher adds it)
        let auto: MapTable = match device {
            Device::Gamepad | Device::Wheel => crate::mapping::stick_dpad(&table, |t| sel.profile.uses(t)).into_iter().collect(),
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
                                (Device::Gun, RowKind::Analog) => "aim the gun across the screen",
                                (Device::Gun, _) => "pull the trigger or press a gun button",
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
                if device == Device::Joystick && ui.button("Forget this device").clicked() {
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
                if let Some(w) = self.joystick.take() {
                    self.set_user(&["input", "devices", &w], None);
                    self.joystick = self.game.as_ref().and_then(|s| s.profile.input.devices.keys().next().cloned());
                }
            }
            None => {}
        }
    }
}

// ---- wheel calibration ----

impl App {
    fn wheels_window(&mut self, ctx: &egui::Context) {
        if !self.show_wheels {
            return;
        }
        let mut open = true;
        egui::Window::new("🚗 Wheel calibration").open(&mut open).default_width(620.0).show(ctx, |ui| {
            if self.wizard.is_some() {
                self.wizard_ui(ui);
            } else {
                self.wheels_list(ui);
            }
        });
        if !open {
            self.show_wheels = false;
            self.wizard = None;
        }
    }

    fn card_window(&mut self, ctx: &egui::Context) {
        if !self.show_card {
            return;
        }
        let mut open = true;
        egui::Window::new("🎫 NESiCA card").open(&mut open).default_width(460.0).show(ctx, |ui| {
            ui.label(
                "The card every NESiCA game reads (inserted with the card input, F4, or by the game \
                 itself when its profile says so). Each game keeps its own progress on it. A new \
                 card id starts over in every game.",
            );
            ui.add_space(8.0);
            egui::Grid::new("card").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Player name");
                ui.add(egui::TextEdit::singleline(&mut self.card.player_name).char_limit(nesica_card::NAME_MAX).hint_text("the game's own"))
                    .on_hover_text("shown by the games whose card has a name (Fighting EX Layer); empty: the game's (NO-NAME)");
                ui.end_row();
                ui.label("Card id");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.card.card_id).char_limit(16).font(egui::TextStyle::Monospace));
                    if ui.button("🔄 New card").on_hover_text("a new card id: no progress in any game").clicked() {
                        self.card.card_id = nesica_card::new_id();
                    }
                });
                ui.end_row();
            });
            let valid = nesica_card::valid_id(&self.card.card_id);
            if !valid {
                ui.colored_label(ui.visuals().error_fg_color, "the card id is 16 digits");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let changed = self.card_saved.as_ref() != Some(&self.card);
                if ui.add_enabled(valid && changed, egui::Button::new("💾 Save")).clicked() {
                    self.status = match nesica_card::save(&self.root, &self.card) {
                        Ok(()) => {
                            self.card_saved = Some(self.card.clone());
                            "saved the NESiCA card".into()
                        }
                        Err(e) => format!("{e:#}"),
                    };
                }
                if changed && let Some(saved) = &self.card_saved {
                    if ui.button("Undo").clicked() {
                        self.card = saved.clone();
                    }
                }
                if self.card_saved.is_none() {
                    ui.label(format!("not saved: the games use card {}", nesica_card::DEFAULT_ID));
                }
            });
        });
        if !open {
            self.show_card = false;
        }
    }

    fn eamuse_window(&mut self, ctx: &egui::Context) {
        if !self.show_eamuse {
            return;
        }
        let mut open = true;
        egui::Window::new("🎫 e-Amusement card").open(&mut open).default_width(460.0).show(ctx, |ui| {
            ui.label(
                "The card every Konami game reads (inserted or touched with the card input, F4). \
                 With a PIN, the loader's e-Amusement server registers the card with it: the game \
                 only asks the PIN (Metal Gear Arcade's reader types it, START touches the card). \
                 Each game keeps its own progress on it. A new card id starts over in every game. \
                 Player 2's card is for games with a reader per player (Hello! Pop'n Music), \
                 inserted with player 2's card input.",
            );
            ui.add_space(8.0);
            egui::Grid::new("eamuse").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Card id");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.eamuse.card_id).char_limit(16).font(egui::TextStyle::Monospace));
                    if ui.button("🔄 New card").on_hover_text("a new card id: no progress in any game").clicked() {
                        self.eamuse.card_id = eamuse_card::new_id();
                    }
                });
                ui.end_row();
                ui.label("PIN");
                ui.add(egui::TextEdit::singleline(&mut self.eamuse.pin).char_limit(4).font(egui::TextStyle::Monospace).hint_text("none"))
                    .on_hover_text("4 digits: the card registered with this PIN; empty: registered on the game's screens");
                ui.end_row();
                ui.label("Player 2 card id");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.eamuse.card2_id).char_limit(16).font(egui::TextStyle::Monospace).hint_text("none"))
                        .on_hover_text("player 2's card, in player 2's reader (games with a reader per player); empty: none");
                    if ui.button("🔄 New card").on_hover_text("a new card id for player 2").clicked() {
                        self.eamuse.card2_id = eamuse_card::new_id();
                    }
                });
                ui.end_row();
                ui.label("Player 2 PIN");
                ui.add(egui::TextEdit::singleline(&mut self.eamuse.pin2).char_limit(4).font(egui::TextStyle::Monospace).hint_text("none"))
                    .on_hover_text("4 digits: player 2's card registered with this PIN");
                ui.end_row();
            });
            let card2_ok = self.eamuse.card2_id.is_empty() || eamuse_card::valid_id(&self.eamuse.card2_id);
            let valid = eamuse_card::valid_id(&self.eamuse.card_id)
                && eamuse_card::valid_pin(&self.eamuse.pin)
                && card2_ok
                && eamuse_card::valid_pin(&self.eamuse.pin2)
                && !self.eamuse.card2_id.eq_ignore_ascii_case(&self.eamuse.card_id);
            if !eamuse_card::valid_id(&self.eamuse.card_id) || !card2_ok {
                ui.colored_label(ui.visuals().error_fg_color, "a card id is 16 hex digits (e.g. E004010027104B1D)");
            }
            if !eamuse_card::valid_pin(&self.eamuse.pin) || !eamuse_card::valid_pin(&self.eamuse.pin2) {
                ui.colored_label(ui.visuals().error_fg_color, "a PIN is 4 digits");
            }
            if self.eamuse.card2_id.eq_ignore_ascii_case(&self.eamuse.card_id) {
                ui.colored_label(ui.visuals().error_fg_color, "player 2's card must differ from player 1's");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let changed = self.eamuse_saved.as_ref() != Some(&self.eamuse);
                if ui.add_enabled(valid && changed, egui::Button::new("💾 Save")).clicked() {
                    self.status = match eamuse_card::save(&self.root, &self.eamuse) {
                        Ok(()) => {
                            self.eamuse_saved = Some(self.eamuse.clone());
                            "saved the e-Amusement card".into()
                        }
                        Err(e) => format!("{e:#}"),
                    };
                }
                if changed && let Some(saved) = &self.eamuse_saved {
                    if ui.button("Undo").clicked() {
                        self.eamuse = saved.clone();
                    }
                }
                if self.eamuse_saved.is_none() {
                    ui.label(format!("not saved: the games use card {}", eamuse_card::DEFAULT_ID));
                }
            });
        });
        if !open {
            self.show_eamuse = false;
        }
    }

    fn save_calibrations(&mut self) {
        self.status = match wheels::save(&self.root, &self.calibrations) {
            Ok(()) => "saved the wheel calibrations".into(),
            Err(e) => format!("{e:#}"),
        };
    }

    fn wheels_list(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "A calibrated wheel is read as a gamepad in every game: steering = left stick X, gas = RT (R2), \
             brake = LT (L2), clutch = right stick down, its buttons as the gamepad buttons you choose, \
             at the gamepad's full ranges. Each game's Wheel tab maps it (by default as its gamepad).",
        );
        ui.add_space(6.0);
        let mut start = None;
        let mut forget = None;
        if self.devices.is_empty() {
            ui.weak("No wheel or joystick connected.");
        }
        for (id, d) in &self.devices {
            let cal = wheels::find(&self.calibrations, &d.name);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.strong(&d.name);
                    match cal {
                        Some(_) => ui.colored_label(egui::Color32::from_rgb(80, 180, 80), "✔ calibrated"),
                        None if d.is_pad => ui.weak("(gamepad)"),
                        None => ui.weak("not calibrated"),
                    };
                    let text = if cal.is_some() { "Calibrate again…" } else { "Calibrate…" };
                    if ui.button(text).clicked() {
                        start = Some((*id, d.name.clone(), d.axes.clone()));
                    }
                    if cal.is_some() && ui.button("Forget").clicked() {
                        forget = Some(d.name.clone());
                    }
                });
                if let Some(cal) = cal {
                    calibration_test(ui, cal, &d.axes, self.wheel_pressed);
                }
            });
        }
        let offline: Vec<String> = self
            .calibrations
            .keys()
            .filter(|n| !self.devices.values().any(|d| d.name.eq_ignore_ascii_case(n)))
            .cloned()
            .collect();
        for n in offline {
            ui.horizontal(|ui| {
                ui.label(format!("{n} (calibrated, not connected)"));
                if ui.button("Forget").clicked() {
                    forget = Some(n.clone());
                }
            });
        }
        if let Some((id, name, axes)) = start {
            self.capture = None;
            self.wizard = Some(Wizard::new(id, name, &axes));
        }
        if let Some(n) = forget {
            self.calibrations.remove(&n);
            self.save_calibrations();
        }
    }

    fn wizard_ui(&mut self, ui: &mut egui::Ui) {
        let Some(w) = &self.wizard else { return };
        let axes = self.devices.get(&w.id).map(|d| d.axes.clone()).unwrap_or_default();
        ui.strong(&w.name);
        if !self.devices.contains_key(&w.id) {
            ui.colored_label(ui.visuals().error_fg_color, "disconnected");
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(w.instruction()).size(16.0));
        if !w.note.is_empty() {
            ui.colored_label(ui.visuals().warn_fg_color, &w.note);
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (i, v) in axes.iter().enumerate() {
                ui.label(format!("Axis {i}"));
                ui.add(egui::ProgressBar::new((*v as f32 + 32768.0) / 65535.0).desired_width(70.0));
            }
        });
        if w.cal != Calibration::default() {
            ui.add_space(4.0);
            calibration_test(ui, &w.cal, &axes, None);
            if !w.cal.buttons.is_empty() {
                let list: Vec<String> = w.cal.buttons.iter().map(|(p, s)| format!("{} = {}", p, bindings::source_label(Device::Joystick, s))).collect();
                ui.weak(list.join(", "));
            }
        }
        ui.add_space(6.0);
        let step = w.step;
        let mut done = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let Some(w) = &mut self.wizard else { return };
            match step {
                Step::Done => done = ui.button(egui::RichText::new("Save").strong()).clicked(),
                Step::Button(_) => {
                    if ui.button("Skip").clicked() {
                        w.skip();
                    }
                }
                Step::Start => {
                    if ui.button("Next").clicked() {
                        w.next();
                    }
                }
                _ => {
                    if ui.button("Next").clicked() {
                        w.next();
                    }
                    if ui.button("Skip").clicked() {
                        w.skip();
                    }
                }
            }
            cancel = ui.button("Cancel").clicked();
        });
        if done {
            if let Some(w) = self.wizard.take() {
                self.calibrations.insert(w.name, w.cal);
                self.save_calibrations();
            }
        } else if cancel {
            self.wizard = None;
        }
    }
}

/// A calibration's gamepad values for the axes now: steering, pedals, the last button.
fn calibration_test(ui: &mut egui::Ui, cal: &Calibration, axes: &[i32], pressed: Option<&str>) {
    let raw = |axis: u8| axes.get(axis as usize).copied().unwrap_or(0);
    ui.horizontal_wrapped(|ui| {
        if let Some(s) = cal.steering {
            let v = s.value(raw(s.axis));
            ui.label("Steering");
            ui.add(egui::ProgressBar::new((v as f32 + 32768.0) / 65535.0).desired_width(120.0).text(v.to_string()));
        }
        for (label, pedal) in [("Gas", cal.gas), ("Brake", cal.brake), ("Clutch", cal.clutch)] {
            if let Some(p) = pedal {
                let v = p.value(raw(p.axis));
                ui.label(label);
                ui.add(egui::ProgressBar::new(v as f32 / 32767.0).desired_width(90.0).text(v.to_string()));
            }
        }
        if let Some(name) = pressed {
            ui.label(format!("last button: {}", bindings::source_label(Device::Gamepad, name)));
        }
    });
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

/// An option's hover text: its environment variable, and its description when it has one.
fn option_hover(opt: &GameOption) -> String {
    match &opt.description {
        Some(d) => format!("{}\n\n{d}", opt.key),
        None => opt.key.clone(),
    }
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
                ui.toggle_value(&mut self.show_wheels, "🚗 Wheel calibration")
                    .on_hover_text("calibrate a wheel and its pedals: then read as a gamepad in every game");
                ui.toggle_value(&mut self.show_card, "🎫 NESiCA card")
                    .on_hover_text("your player card in every NESiCA game: player name and card id");
                ui.toggle_value(&mut self.show_eamuse, "🎫 e-Amusement card")
                    .on_hover_text("your player card in every Konami game: card id and PIN");
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
        self.wheels_window(ui.ctx());
        self.card_window(ui.ctx());
        self.eamuse_window(ui.ctx());
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
