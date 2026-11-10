//! XInput: one connected pad per player, built from its virtual stick.
//!
//! Galaxy Rangers' `ArcadeControls` reads `XInputGetState` every frame (`Controls.ini`
//! `[PlayerN]`, the dump's defaults): the left stick is the aim, absolute (-1 left/bottom ..
//! 1 right/top of the screen, Unity's y going up), the right trigger shoots, LB reloads, RB
//! changes the weapon, START starts and BACK inserts a coin (on their press).
//!
//! The gun position is the stick's `lx`/`ly` (`ly` +down, inverted). A shot off the screen
//! reloads, as the cabinet's guns: the trigger is then LB, not a shot (`offscreen`, the
//! launcher's gun off-screen source).
//!
//! Natives for `native_map`: `trigger reload change start coin offscreen` (every player), and
//! player 1's operator keys `menu menu_up menu_down menu_left menu_right` (`F2` opens the
//! settings, the arrows choose and change them; Enter = right). Unmapped by default:
//! `button_a`, `button_b`, `left_trigger`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use wal_protocol::{Axis, MAX_PLAYERS};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Change,
    Start,
    Coin,
    Offscreen,
    Menu,
    MenuUp,
    MenuDown,
    MenuLeft,
    MenuRight,
    ButtonA,
    ButtonB,
    LeftTrigger,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("reload", Native::Reload),
    ("change", Native::Change),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("offscreen", Native::Offscreen),
    ("menu", Native::Menu),
    ("menu_up", Native::MenuUp),
    ("menu_down", Native::MenuDown),
    ("menu_left", Native::MenuLeft),
    ("menu_right", Native::MenuRight),
    ("button_a", Native::ButtonA),
    ("button_b", Native::ButtonB),
    ("left_trigger", Native::LeftTrigger),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "change"),
    ("b3", "reload"),
    ("b6", "offscreen"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "menu"),
    ("up", "menu_up"),
    ("down", "menu_down"),
    ("left", "menu_left"),
    ("right", "menu_right"),
];

/// Player 1's operator keys: (native, virtual key).
const KEYS: [(Native, i32); 5] = [(Native::Menu, 0x71), (Native::MenuUp, 0x26), (Native::MenuDown, 0x28), (Native::MenuLeft, 0x25), (Native::MenuRight, 0x27)];

const ERROR_SUCCESS: u32 = 0;
const ERROR_EMPTY: u32 = 4306;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

const XINPUT_GAMEPAD_START: u16 = 0x0010;
const XINPUT_GAMEPAD_BACK: u16 = 0x0020;
const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
const XINPUT_GAMEPAD_A: u16 = 0x1000;
const XINPUT_GAMEPAD_B: u16 = 0x2000;

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn pressed(player: usize) -> Vec<Native> {
    let mut natives = Vec::new();
    map().for_each_pressed(&input(player), |n| natives.push(n));
    natives
}

/// XINPUT_GAMEPAD: buttons, left and right triggers, lx ly rx ry.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct Gamepad {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

fn gamepad(player: usize) -> Gamepad {
    let stick = input(player);
    let natives = pressed(player);
    let on = |n| natives.contains(&n);
    let offscreen = on(Native::Offscreen);
    let mut pad = Gamepad { lx: stick.axis(Axis::LeftX), ly: stick.axis(Axis::LeftY).saturating_neg(), ..Default::default() };
    if RELOAD_LEFT_TRIGGER.load(Ordering::Relaxed) {
        // the gun is not the stick there (its menus' navigation)
        (pad.lx, pad.ly) = (0, 0);
    }
    if on(Native::Trigger) && !offscreen {
        pad.right_trigger = 255;
    }
    if on(Native::Reload) || (on(Native::Trigger) && offscreen) {
        if RELOAD_LEFT_TRIGGER.load(Ordering::Relaxed) {
            pad.left_trigger = 255;
        } else {
            pad.buttons |= XINPUT_GAMEPAD_LEFT_SHOULDER;
        }
    }
    if on(Native::LeftTrigger) {
        pad.left_trigger = 255;
    }
    if on(Native::ButtonA) {
        pad.buttons |= XINPUT_GAMEPAD_A;
    }
    if on(Native::ButtonB) {
        pad.buttons |= XINPUT_GAMEPAD_B;
    }
    if on(Native::Change) {
        pad.buttons |= XINPUT_GAMEPAD_RIGHT_SHOULDER;
    }
    if on(Native::Start) {
        pad.buttons |= XINPUT_GAMEPAD_START;
    }
    // (the payload's own coins there)
    if on(Native::Coin) && !RELOAD_LEFT_TRIGGER.load(Ordering::Relaxed) {
        pad.buttons |= XINPUT_GAMEPAD_BACK;
    }
    pad
}

/// Whether the game sees virtual key `vk` held (the filtered keyboard's only keys).
pub(crate) fn key_held(vk: i32) -> bool {
    KEYS.iter().any(|(n, k)| *k == vk && pressed(0).contains(n))
}

/// The reload (and a shot off the screen) on the left trigger instead of LB, the stick centered,
/// one pad: player 2's (The House of the Dead 2: Remake: the payload places its crosshairs,
/// player 1 is its mouse).
static RELOAD_LEFT_TRIGGER: AtomicBool = AtomicBool::new(false);

pub(crate) fn reload_on_left_trigger() {
    RELOAD_LEFT_TRIGGER.store(true, Ordering::Relaxed);
}

/// Whether `player`'s trigger is held (on or off the screen).
pub(crate) fn trigger_held(player: usize) -> bool {
    pressed(player).contains(&Native::Trigger)
}

/// Whether `player` holds native `name` (`NATIVES`).
pub(crate) fn held(player: usize, name: &str) -> bool {
    NATIVES.iter().find(|(n, _)| *n == name).is_some_and(|(_, native)| pressed(player).contains(native))
}

/// Whether `player`'s reload button is held.
pub(crate) fn reload_held(player: usize) -> bool {
    pressed(player).contains(&Native::Reload)
}

/// Whether `player`'s gun is off the screen.
pub(crate) fn offscreen_held(player: usize) -> bool {
    pressed(player).contains(&Native::Offscreen)
}

/// No pad connected (the DemulShooter plugin games: their guns are not pads).
static DISCONNECTED: AtomicBool = AtomicBool::new(false);

pub(crate) fn disconnect() {
    DISCONNECTED.store(true, Ordering::Relaxed);
}

fn absent(index: u32) -> bool {
    let only_player_2 = RELOAD_LEFT_TRIGGER.load(Ordering::Relaxed);
    index as usize >= if only_player_2 { 1 } else { MAX_PLAYERS } || DISCONNECTED.load(Ordering::Relaxed)
}

/// The player of XInput pad `index` (with [`reload_on_left_trigger`]: one pad, player 2's).
fn player_of(index: u32) -> usize {
    index as usize + RELOAD_LEFT_TRIGGER.load(Ordering::Relaxed) as usize
}

/// Last pad of each player and its packet number (changed with the pad).
static LAST: Mutex<[(Gamepad, u32); MAX_PLAYERS]> = Mutex::new([(Gamepad { buttons: 0, left_trigger: 0, right_trigger: 0, lx: 0, ly: 0, rx: 0, ry: 0 }, 0); MAX_PLAYERS]);

/// XINPUT_STATE: packet number, gamepad.
#[repr(C)]
pub struct State {
    packet: u32,
    gamepad: Gamepad,
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XInputGetState(index: u32, state: *mut State) -> u32 {
    let player = player_of(index);
    if absent(index) {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    let pad = gamepad(player);
    let packet = {
        let mut last = LAST.lock().unwrap();
        let (previous, packet) = &mut last[player];
        if *previous != pad {
            if player == 0 && previous.buttons != pad.buttons {
                log!("pads: P1 buttons {:#06x}", pad.buttons);
            }
            *previous = pad;
            *packet = packet.wrapping_add(1);
        }
        *packet
    };
    if !state.is_null() {
        unsafe { state.write(State { packet, gamepad: pad }) };
    }
    ERROR_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XInputGetStateEx(index: u32, state: *mut State) -> u32 {
    unsafe { XInputGetState(index, state) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XInputSetState(index: u32, _vibration: *mut c_void) -> u32 {
    if absent(index) { ERROR_DEVICE_NOT_CONNECTED } else { ERROR_SUCCESS }
}

/// XINPUT_CAPABILITIES: type, subtype, flags, the gamepad's supported inputs, the motors.
#[repr(C)]
pub struct Capabilities {
    kind: u8,
    sub_kind: u8,
    flags: u16,
    gamepad: Gamepad,
    left_motor: u16,
    right_motor: u16,
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XInputGetCapabilities(index: u32, _flags: u32, caps: *mut Capabilities) -> u32 {
    if absent(index) {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    if !caps.is_null() {
        let gamepad = Gamepad { buttons: 0xF3FF, left_trigger: 255, right_trigger: 255, lx: -64, ly: -64, rx: -64, ry: -64 };
        // XINPUT_DEVTYPE_GAMEPAD, XINPUT_DEVSUBTYPE_GAMEPAD
        unsafe { caps.write(Capabilities { kind: 1, sub_kind: 1, flags: 0, gamepad, left_motor: 0xFFFF, right_motor: 0xFFFF }) };
    }
    ERROR_SUCCESS
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputEnable(_enable: i32) {}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XInputGetDSoundAudioDeviceGuids(index: u32, render: *mut [u8; 16], capture: *mut [u8; 16]) -> u32 {
    if absent(index) {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    for guid in [render, capture] {
        if !guid.is_null() {
            unsafe { guid.write([0; 16]) };
        }
    }
    ERROR_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XInputGetBatteryInformation(index: u32, _kind: u8, info: *mut [u8; 2]) -> u32 {
    if absent(index) {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    if !info.is_null() {
        // BATTERY_TYPE_WIRED, BATTERY_LEVEL_FULL
        unsafe { info.write([1, 3]) };
    }
    ERROR_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn XInputGetKeystroke(index: u32, _reserved: u32, _keystroke: *mut c_void) -> u32 {
    if (index as usize >= MAX_PLAYERS && index != 0xFF) || DISCONNECTED.load(Ordering::Relaxed) { ERROR_DEVICE_NOT_CONNECTED } else { ERROR_EMPTY }
}
