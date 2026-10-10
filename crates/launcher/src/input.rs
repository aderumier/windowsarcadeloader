//! Physical input devices: SDL3 gamepads/joysticks/wheels, evdev keyboards, lightguns and
//! mice (`guns.rs`).
//!
//! Every device is a [`DeviceState`] bound to a player; the hub merges them into the
//! virtual arcade sticks. Force feedback outputs of the game go back to the player's devices:
//! wheels through evdev (`ffb.rs`), gamepads as SDL rumble.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::{Context, Result};
use sdl3::event::Event;
use sdl3::gamepad::Gamepad;
use sdl3::joystick::{HatState, Joystick, JoystickId};
use sdl3::{GamepadSubsystem, JoystickSubsystem};
use wal_protocol::{InputFrame, MAX_PLAYERS, Output, output};

use crate::config::Profile;
use crate::ffb;
use crate::guns::{self, PointerEvent};
use crate::mapping::{self, DeviceState, Mapping, RawKey, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DevKey {
    Sdl(u32),
    Keyboard(usize),
    Pointer(usize),
}

enum Handle {
    Pad(Gamepad),
    Joy(#[allow(dead_code)] Joystick),
}

/// A gamepad's force feedback state (constant force, vibration), played as rumble.
#[derive(Default, Clone, Copy, PartialEq)]
struct Rumble {
    constant: i32,
    vibration: i32,
}

pub struct Hub<'a> {
    config: &'a Profile,
    gamepad_map: Mapping,
    exit_sources: Vec<Source>,
    /// evdev key codes quitting the game.
    exit_keys: Vec<u16>,
    devices: HashMap<DevKey, DeviceState>,
    handles: HashMap<u32, Handle>,
    wheels: HashMap<u32, ffb::Wheel>,
    rumbles: HashMap<u32, Rumble>,
    _sdl: sdl3::Sdl,
    gamepads: GamepadSubsystem,
    joysticks: JoystickSubsystem,
    pump: sdl3::EventPump,
    keys: Option<Receiver<(u16, i32)>>,
    pointers: Option<Receiver<PointerEvent>>,
    pub exit_requested: bool,
    /// Print device events (input test mode).
    pub verbose: bool,
}

impl<'a> Hub<'a> {
    pub fn new(config: &'a Profile) -> Result<Self> {
        let gamepad_map =
            mapping::compile(&config.input.gamepad, mapping::parse_pad_source).context("input.gamepad")?;
        let exit_sources = config
            .input
            .exit_combo
            .iter()
            .map(|n| mapping::parse_pad_source(n).with_context(|| format!("exit_combo: unknown button '{n}'")))
            .collect::<Result<_>>()?;

        let exit_keys = config
            .input
            .exit_keys
            .iter()
            .map(|n| {
                n.to_ascii_uppercase()
                    .parse::<evdev::KeyCode>()
                    .map(|k| k.code())
                    .map_err(|_| anyhow::anyhow!("exit_keys: unknown key '{n}'"))
            })
            .collect::<Result<Vec<_>>>()?;

        let mut devices = HashMap::new();
        // keyboards read for their mapping, and for the exit keys
        let keys = if config.input.keyboard_enabled || !exit_keys.is_empty() {
            let tables = if config.input.keyboard_enabled { config.input.keyboard.iter().collect() } else { Vec::new() };
            for (name, table) in tables {
                let player = name
                    .strip_prefix('p')
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| (1..=MAX_PLAYERS).contains(n))
                    .with_context(|| format!("input.keyboard.{name}: expected p1..p{MAX_PLAYERS}"))?;
                let m = mapping::compile(table, mapping::parse_key_source)
                    .with_context(|| format!("input.keyboard.{name}"))?;
                devices.insert(DevKey::Keyboard(player - 1), DeviceState::new(player - 1, m));
            }
            Some(start_keyboards().0)
        } else {
            None
        };

        let pointers = if config.input.guns_enabled {
            let m = mapping::compile(&config.input.gun, mapping::parse_gun_source).context("input.gun")?;
            let (tx, rx) = channel();
            let found = guns::start(config.input.guns_mouse, config.input.guns_mice, config.input.mouse_screen, tx);
            if found.is_empty() {
                eprintln!("input: no lightgun or mouse found");
            }
            for (i, p) in found.iter().enumerate() {
                let kind = if p.is_gun { "lightgun" } else { "mouse" };
                eprintln!("input: {kind} '{}' ({}) -> player {}", p.name, p.path.display(), p.player + 1);
                devices.insert(DevKey::Pointer(i), DeviceState::new(p.player, m.clone()));
            }
            Some(rx)
        } else {
            None
        };

        // gamepads/wheels are read in the background while the game window has the focus
        sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        let sdl = sdl3::init().map_err(|e| anyhow::anyhow!("SDL init: {e}"))?;
        let gamepads = sdl.gamepad().map_err(|e| anyhow::anyhow!("SDL gamepad: {e}"))?;
        let joysticks = sdl.joystick().map_err(|e| anyhow::anyhow!("SDL joystick: {e}"))?;
        let pump = sdl.event_pump().map_err(|e| anyhow::anyhow!("SDL events: {e}"))?;

        Ok(Hub {
            config,
            gamepad_map,
            exit_sources,
            exit_keys,
            devices,
            handles: HashMap::new(),
            wheels: HashMap::new(),
            rumbles: HashMap::new(),
            _sdl: sdl,
            gamepads,
            joysticks,
            pump,
            keys,
            pointers,
            exit_requested: false,
            verbose: false,
        })
    }

    /// Processes pending events, waiting at most `timeout`.
    pub fn poll(&mut self, timeout: Duration) {
        if let Some(ev) = self.pump.wait_event_timeout(timeout) {
            self.handle(ev);
            while let Some(ev) = self.pump.poll_event() {
                self.handle(ev);
            }
        }
        if let Some(rx) = &self.keys {
            let pending: Vec<_> = rx.try_iter().collect();
            for (code, value) in pending {
                if value == 1 && self.exit_keys.contains(&code) {
                    if !self.exit_requested {
                        eprintln!("input: exit key pressed");
                    }
                    self.exit_requested = true;
                }
                for (key, dev) in self.devices.iter_mut() {
                    if matches!(key, DevKey::Keyboard(_)) {
                        dev.raw.insert(RawKey::Key(code), value);
                    }
                }
            }
        }
        if let Some(rx) = &self.pointers {
            let pending: Vec<_> = rx.try_iter().collect();
            for (index, key, value) in pending {
                if let Some(dev) = self.devices.get_mut(&DevKey::Pointer(index)) {
                    dev.raw.insert(key, value);
                }
            }
        }
    }

    pub fn frame(&self) -> InputFrame {
        mapping::merge(self.devices.values(), self.config.input.deadzone, self.config.input.stick_as_dpad)
    }

    /// A force feedback output of the game, played on the player's devices: wheels (evdev
    /// effects), else gamepads (rumble: the strong motor as hard as the vibration, the weak one
    /// as the constant force pushes).
    pub fn output(&mut self, o: Output) {
        if !self.config.input.ffb.enabled || !matches!(o.id, output::FFB_CONSTANT | output::FFB_SPRING | output::FFB_VIBRATION) {
            return;
        }
        let ids: Vec<u32> = self
            .devices
            .iter()
            .filter_map(|(k, d)| match k {
                DevKey::Sdl(id) if d.player == o.player as usize => Some(*id),
                _ => None,
            })
            .collect();
        let gain = self.config.input.ffb.gain.min(100) as i64;
        for id in ids {
            if let Some(wheel) = self.wheels.get_mut(&id) {
                wheel.set(o.id, o.value);
                continue;
            }
            let Some(Handle::Pad(pad)) = self.handles.get_mut(&id) else { continue };
            let r = self.rumbles.entry(id).or_default();
            let before = *r;
            match o.id {
                output::FFB_CONSTANT => r.constant = o.value.abs(),
                output::FFB_VIBRATION => r.vibration = o.value.max(0),
                _ => continue,
            }
            let scale = |v: i32| (v.min(output::FFB_MAX) as i64 * 0xFFFF / output::FFB_MAX as i64 * gain / 100) as u16;
            // refreshed by the game every second: the rumble lasts a bit longer
            if *r != before || r.constant != 0 || r.vibration != 0 {
                let _ = pad.set_rumble(scale(r.vibration), scale(r.constant), 1500);
            }
        }
    }

    fn free_player(&self) -> usize {
        (0..MAX_PLAYERS)
            .find(|p| !self.devices.iter().any(|(k, d)| matches!(k, DevKey::Sdl(_)) && d.player == *p))
            .unwrap_or(MAX_PLAYERS)
    }

    fn add(&mut self, id: JoystickId) {
        let raw_id = id.raw();
        if self.handles.contains_key(&raw_id) {
            return;
        }
        let is_pad = self.gamepads.is_gamepad(id);
        let pad_name = if is_pad { self.gamepads.name_for_id(id).ok() } else { None };
        let joy = if is_pad { None } else { self.joysticks.open(id).ok() };
        let name = pad_name.or_else(|| joy.as_ref().map(|j| j.name())).unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        let dev_cfg = self.config.input.devices.iter().find(|(m, _)| lower.contains(&m.to_ascii_lowercase()));
        let dev_name = dev_cfg.map(|(m, _)| m.clone()).unwrap_or_default();
        let dev_cfg = dev_cfg.map(|(_, d)| d);
        let player = dev_cfg.and_then(|d| d.player).map(|p| p.saturating_sub(1)).unwrap_or_else(|| self.free_player());

        let (handle, mapping) = if is_pad && !dev_cfg.is_some_and(|d| d.raw) {
            let pad = match self.gamepads.open(id) {
                Ok(p) => p,
                Err(e) => return eprintln!("input: cannot open gamepad {name}: {e}"),
            };
            let mut mapping = self.gamepad_map.clone();
            if let Some(d) = dev_cfg {
                match mapping::compile(&d.map, mapping::parse_pad_source) {
                    Ok(extra) => mapping.extend(extra),
                    Err(e) => eprintln!("input: device '{dev_name}': {e:#}"),
                }
            }
            (Handle::Pad(pad), mapping)
        } else {
            let Some(d) = dev_cfg else {
                return eprintln!("input: ignoring joystick '{name}' (no input.devices entry matches it)");
            };
            let joy = match joy.map(Ok).unwrap_or_else(|| self.joysticks.open(id)) {
                Ok(j) => j,
                Err(e) => return eprintln!("input: cannot open joystick {name}: {e}"),
            };
            match mapping::compile(&d.map, mapping::parse_joy_source) {
                Ok(m) => (Handle::Joy(joy), m),
                Err(e) => return eprintln!("input: device '{dev_name}': {e:#}"),
            }
        };

        let mut state = DeviceState::new(player, mapping);
        if let Handle::Joy(j) = &handle {
            // pedals rest at a non zero position: read the initial axes
            for i in 0..j.num_axes() {
                if let Ok(v) = j.axis(i) {
                    state.raw.insert(RawKey::JoyAxis(i as u8), v as i32);
                }
            }
        }
        let kind = if matches!(handle, Handle::Pad(_)) { "gamepad" } else { "joystick" };
        eprintln!("input: {kind} '{name}' -> player {}", player + 1);
        if self.config.input.ffb.enabled {
            // the device's evdev node: a wheel when it supports a constant force
            let path = unsafe { sdl3::sys::joystick::SDL_GetJoystickPathForID(sdl3::sys::joystick::SDL_JoystickID(raw_id)) };
            if !path.is_null() {
                let path = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().into_owned();
                if let Some(wheel) = ffb::Wheel::open(&path, &self.config.input.ffb) {
                    self.wheels.insert(raw_id, wheel);
                }
            }
        }
        self.devices.insert(DevKey::Sdl(raw_id), state);
        self.handles.insert(raw_id, handle);
    }

    fn remove(&mut self, id: JoystickId) {
        if self.handles.remove(&id.raw()).is_some() {
            self.devices.remove(&DevKey::Sdl(id.raw()));
            self.wheels.remove(&id.raw());
            self.rumbles.remove(&id.raw());
            eprintln!("input: device {} removed", id.raw());
        }
    }

    fn set(&mut self, id: JoystickId, key: RawKey, value: i32, from_pad: bool) {
        // SDL reports gamepads both as gamepad and joystick events: keep the right one
        let is_pad = matches!(self.handles.get(&id.raw()), Some(Handle::Pad(_)));
        if is_pad != from_pad {
            return;
        }
        if let Some(dev) = self.devices.get_mut(&DevKey::Sdl(id.raw())) {
            if self.verbose {
                eprintln!("input: device {} {key:?} = {value}", id.raw());
            }
            dev.raw.insert(key, value);
            if from_pad && dev.all_pressed(&self.exit_sources) {
                self.exit_requested = true;
            }
        }
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::JoyDeviceAdded { which, .. } => self.add(which),
            Event::JoyDeviceRemoved { which, .. } => self.remove(which),
            Event::GamepadButtonDown { which, button, .. } => self.set(which, RawKey::PadButton(button as i32), 1, true),
            Event::GamepadButtonUp { which, button, .. } => self.set(which, RawKey::PadButton(button as i32), 0, true),
            Event::GamepadAxisMotion { which, axis, value, .. } => {
                self.set(which, RawKey::PadAxis(axis as i32), value as i32, true)
            }
            Event::JoyButtonDown { which, button_idx, .. } => self.set(which, RawKey::JoyButton(button_idx), 1, false),
            Event::JoyButtonUp { which, button_idx, .. } => self.set(which, RawKey::JoyButton(button_idx), 0, false),
            Event::JoyAxisMotion { which, axis_idx, value, .. } => {
                self.set(which, RawKey::JoyAxis(axis_idx), value as i32, false)
            }
            Event::JoyHatMotion { which, hat_idx, state, .. } => {
                self.set(which, RawKey::JoyHat(hat_idx), hat_bits(state), false)
            }
            _ => {}
        }
    }
}

pub fn hat_bits(state: HatState) -> i32 {
    match state {
        HatState::Centered => 0,
        HatState::Up => 0x01,
        HatState::Right => 0x02,
        HatState::Down => 0x04,
        HatState::Left => 0x08,
        HatState::RightUp => 0x03,
        HatState::RightDown => 0x06,
        HatState::LeftUp => 0x09,
        HatState::LeftDown => 0x0C,
    }
}

/// Reads every keyboard with evdev (no grab: the game window still gets its keys): (key code,
/// value) events, and the number of keyboards.
pub fn start_keyboards() -> (Receiver<(u16, i32)>, usize) {
    let (tx, rx) = channel();
    let mut count = 0;
    for (path, dev) in evdev::enumerate() {
        let is_keyboard = dev
            .supported_keys()
            .is_some_and(|k| k.contains(evdev::KeyCode::KEY_A) && k.contains(evdev::KeyCode::KEY_ENTER));
        if is_keyboard {
            eprintln!("input: keyboard '{}' ({})", dev.name().unwrap_or("?"), path.display());
            let tx: Sender<(u16, i32)> = tx.clone();
            std::thread::spawn(move || read_keyboard(dev, tx));
            count += 1;
        }
    }
    (rx, count)
}

fn read_keyboard(mut dev: evdev::Device, tx: Sender<(u16, i32)>) {
    while let Ok(events) = dev.fetch_events() {
        for ev in events {
            if let evdev::EventSummary::Key(_, code, value) = ev.destructure() {
                // 2 = autorepeat
                if value != 2 && tx.send((code.code(), value)).is_err() {
                    return;
                }
            }
        }
    }
}
