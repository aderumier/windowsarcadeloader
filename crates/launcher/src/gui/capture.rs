//! Physical inputs of the GUI: SDL3 gamepads/joysticks and evdev keyboards, read in background
//! threads as the launcher reads them, sent to the window as events (mapping capture).

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use sdl3::event::Event;
use sdl3::gamepad::{Axis as PadAxis, Button as PadButton, Gamepad};
use sdl3::joystick::Joystick;

use crate::input;

pub enum InputEvent {
    /// The number of keyboards read (0: no access to /dev/input).
    Keyboards(usize),
    /// evdev key code, pressed.
    Key(u16, bool),
    /// A device connected: SDL id, name, SDL knows it as a gamepad, its axes at rest.
    Added { id: u32, name: String, is_pad: bool, axes: Vec<i32> },
    Removed(u32),
    /// Any gamepad (SDL positional names of the profiles).
    PadButton { name: &'static str, down: bool },
    PadAxis { name: &'static str, value: i32 },
    /// Raw joystick (wheels).
    JoyButton { id: u32, index: u8, down: bool },
    JoyAxis { id: u32, index: u8, value: i32 },
    JoyHat { id: u32, index: u8, bits: u8 },
}

/// Starts the readers; `wake` is called after each event (repaint).
pub fn start(wake: impl Fn() + Send + Clone + 'static) -> Receiver<InputEvent> {
    let (tx, rx) = channel();
    {
        let (tx, wake) = (tx.clone(), wake.clone());
        std::thread::spawn(move || {
            let (keys, count) = input::start_keyboards();
            let _ = tx.send(InputEvent::Keyboards(count));
            wake();
            for (code, value) in keys {
                if tx.send(InputEvent::Key(code, value != 0)).is_err() {
                    return;
                }
                wake();
            }
        });
    }
    std::thread::spawn(move || {
        if let Err(e) = read_sdl(&tx, &wake) {
            eprintln!("gui: SDL input: {e}");
        }
    });
    rx
}

enum Handle {
    Pad(#[allow(dead_code)] Gamepad, #[allow(dead_code)] Joystick),
    Joy(#[allow(dead_code)] Joystick),
}

fn read_sdl(tx: &Sender<InputEvent>, wake: &impl Fn()) -> Result<(), String> {
    sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
    // SIGINT/SIGTERM would only become SDL quit events: the window must quit on them
    sdl3::hint::set("SDL_NO_SIGNAL_HANDLERS", "1");
    let sdl = sdl3::init().map_err(|e| e.to_string())?;
    let gamepads = sdl.gamepad().map_err(|e| e.to_string())?;
    let joysticks = sdl.joystick().map_err(|e| e.to_string())?;
    let mut pump = sdl.event_pump().map_err(|e| e.to_string())?;
    let mut handles: HashMap<u32, Handle> = HashMap::new();
    loop {
        let Some(first) = pump.wait_event_timeout(Duration::from_millis(100)) else { continue };
        for ev in std::iter::once(first).chain(std::iter::from_fn(|| pump.poll_event())) {
            let out = match ev {
                Event::JoyDeviceAdded { which, .. } => {
                    let id = which.raw();
                    if handles.contains_key(&id) {
                        continue;
                    }
                    // raw joystick events of every device (wheel tab), gamepad events of pads
                    let Ok(joy) = joysticks.open(which) else { continue };
                    let name = joy.name();
                    let axes = (0..joy.num_axes()).map(|i| joy.axis(i).map(i32::from).unwrap_or(0)).collect();
                    let is_pad = gamepads.is_gamepad(which);
                    let handle = match is_pad.then(|| gamepads.open(which).ok()).flatten() {
                        Some(pad) => Handle::Pad(pad, joy),
                        None => Handle::Joy(joy),
                    };
                    let is_pad = matches!(handle, Handle::Pad(..));
                    handles.insert(id, handle);
                    InputEvent::Added { id, name, is_pad, axes }
                }
                Event::JoyDeviceRemoved { which, .. } => {
                    handles.remove(&which.raw());
                    InputEvent::Removed(which.raw())
                }
                Event::GamepadButtonDown { button, .. } | Event::GamepadButtonUp { button, .. } => {
                    let down = matches!(ev, Event::GamepadButtonDown { .. });
                    let Some(name) = pad_button_name(button) else { continue };
                    InputEvent::PadButton { name, down }
                }
                Event::GamepadAxisMotion { axis, value, .. } => {
                    InputEvent::PadAxis { name: pad_axis_name(axis), value: value as i32 }
                }
                Event::JoyButtonDown { which, button_idx, .. } => {
                    InputEvent::JoyButton { id: which.raw(), index: button_idx, down: true }
                }
                Event::JoyButtonUp { which, button_idx, .. } => {
                    InputEvent::JoyButton { id: which.raw(), index: button_idx, down: false }
                }
                Event::JoyAxisMotion { which, axis_idx, value, .. } => {
                    InputEvent::JoyAxis { id: which.raw(), index: axis_idx, value: value as i32 }
                }
                Event::JoyHatMotion { which, hat_idx, state, .. } => {
                    InputEvent::JoyHat { id: which.raw(), index: hat_idx, bits: input::hat_bits(state) as u8 }
                }
                _ => continue,
            };
            if tx.send(out).is_err() {
                return Ok(());
            }
            wake();
        }
    }
}

/// The profiles' gamepad source name of an SDL button.
fn pad_button_name(b: PadButton) -> Option<&'static str> {
    use PadButton::*;
    Some(match b {
        South => "a",
        East => "b",
        West => "x",
        North => "y",
        Back => "back",
        Guide => "guide",
        Start => "start",
        LeftStick => "l3",
        RightStick => "r3",
        LeftShoulder => "l1",
        RightShoulder => "r1",
        DPadUp => "dpup",
        DPadDown => "dpdown",
        DPadLeft => "dpleft",
        DPadRight => "dpright",
        Misc1 => "misc1",
        RightPaddle1 => "paddle1",
        LeftPaddle1 => "paddle2",
        RightPaddle2 => "paddle3",
        LeftPaddle2 => "paddle4",
        Touchpad => "touchpad",
        _ => return None,
    })
}

fn pad_axis_name(a: PadAxis) -> &'static str {
    match a {
        PadAxis::LeftX => "leftx",
        PadAxis::LeftY => "lefty",
        PadAxis::RightX => "rightx",
        PadAxis::RightY => "righty",
        PadAxis::TriggerLeft => "l2",
        PadAxis::TriggerRight => "r2",
    }
}
