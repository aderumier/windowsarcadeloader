//! Subway Surfers Arcade (an arcade conversion of Kiloo / SYBO's game: `SubwaySurfers.exe`,
//! Unity 2022.3, Mono, 64-bit): `WAL_ARCADEPC_BOARD=subway`.
//!
//! The cabinet's controls are keys read with Unity's `Input.GetKeyDown`:
//! - **Run.** `Game.HandleDebugControls` (called by `Game.Update` in this build): the arrows (or
//!   the keypad's 8 / 2 / 4 / 6) are swipes (`CharacterState.HandleSwipe`: up jumps, down rolls,
//!   left / right change lanes). The character select (`TrocaModeloPersonagem`): left / right.
//! - **Credits.** `InsiraCreditos.Update`: `5` released (or keypad 5) adds a credit, `1` (or
//!   keypad 1) starts a run with one. Escape quits the game: never posted.
//! - **Hoverboard.** Only the touch screen's double tap (`Game.HandleControls`: Unity's left
//!   mouse button, `HandleTap`, `HandleDoubleTap`).
//! - **Lamps.** `ArduinoController`: an Arduino on COM5 (115200 bauds) lit with `<pin>.1x` /
//!   `<pin>.0x` writes, nothing read. The port is a silent device of the payload's (Mono's
//!   `SerialPort` `DllImport`s): it never reaches a real serial port.
//!
//! The payload posts the keys and the taps (`payload-common`'s `input_filter`: the real keyboard
//! and mouse never reach the game, nor its debug keys). The left stick presses the arrows too:
//! tilted past half way, once in its main direction (again once it is back near the middle).
//! Free play (the common `WAL_FREEPLAY`): start inserts a coin, then starts.
//!
//! Natives: `up`, `down`, `left`, `right` (the arrows), `start`, `coin`, `hoverboard` (player 1).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse, post_raw_buttons};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports, serial};
use wal_protocol::{Axis, StickState};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;
const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;
const VK_1: u16 = 0x31;
const VK_5: u16 = 0x35;

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;
/// Raw mouse button transitions.
const RI_MOUSE_LEFT_BUTTON_DOWN: u16 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_UP: u16 = 0x0002;

/// The lamps' Arduino.
const PORT: &str = "COM5";

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A key: its virtual key.
    Key(u16),
    Start,
    Hoverboard,
}

const NATIVES: &[(&str, Native)] = &[
    ("up", Native::Key(VK_UP)),
    ("down", Native::Key(VK_DOWN)),
    ("left", Native::Key(VK_LEFT)),
    ("right", Native::Key(VK_RIGHT)),
    ("start", Native::Start),
    ("coin", Native::Key(VK_5)),
    ("hoverboard", Native::Hoverboard),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "up"),
    ("b2", "hoverboard"),
    ("start", "start"),
    ("coin", "coin"),
];

/// The left stick's tilt that presses an arrow, and the one under which it presses again.
const STICK_PRESS: i32 = 16384;
const STICK_REARM: i32 = 9830;
/// A step of a gesture: a frame or two of the game.
const STEP: Duration = Duration::from_millis(30);

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keys held (bit n: virtual key n), for the game's key state polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn hold(keys: &mut [u64; 4], vk: u16) {
    keys[vk as usize / 64] |= 1 << (vk % 64);
}

/// The left stick's arrow past [`STICK_PRESS`] (its main axis), `None` under it. `armed`:
/// whether it came back under [`STICK_REARM`] since its last press.
fn stick_arrow(stick: &StickState, armed: &mut bool) -> Option<u16> {
    let (x, y) = (stick.axis(Axis::LeftX) as i32, stick.axis(Axis::LeftY) as i32);
    let tilt = x.abs().max(y.abs());
    if tilt < STICK_REARM {
        *armed = true;
    }
    if !*armed || tilt < STICK_PRESS {
        return None;
    }
    *armed = false;
    // `ly` goes down
    Some(match (x.abs() > y.abs(), x > 0, y > 0) {
        (true, true, _) => VK_RIGHT,
        (true, false, _) => VK_LEFT,
        (false, _, true) => VK_DOWN,
        (false, _, false) => VK_UP,
    })
}

/// The left button pressed or released at client point `at`, as a message and a raw report.
fn button(hwnd: HWND, down: bool, at: (i32, i32)) {
    if down {
        post_mouse(hwnd, WM_LBUTTONDOWN, at.0, at.1, MK_LBUTTON);
        post_raw_buttons(hwnd, RI_MOUSE_LEFT_BUTTON_DOWN);
    } else {
        post_mouse(hwnd, WM_LBUTTONUP, at.0, at.1, 0);
        post_raw_buttons(hwnd, RI_MOUSE_LEFT_BUTTON_UP);
    }
}

/// The hoverboard: two taps in the middle of the picture.
fn double_tap(hwnd: HWND) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd, &mut r) };
    let at = (r.right / 2, r.bottom / 2);
    post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, 0);
    std::thread::sleep(STEP);
    for _ in 0..2 {
        button(hwnd, true, at);
        std::thread::sleep(STEP);
        button(hwnd, false, at);
        std::thread::sleep(STEP);
    }
}

/// A key pressed and released, a few frames each.
fn pulse(hwnd: HWND, vk: u16) {
    post_key(hwnd, vk, true);
    std::thread::sleep(STEP * 2);
    post_key(hwnd, vk, false);
    std::thread::sleep(STEP * 2);
}

fn input_thread() {
    let free_play = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(false);
    let (mut keys_was, mut start_was, mut hoverboard_was, mut armed) = ([0u64; 4], false, false, true);
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let stick = input(0);
        let mut keys = [0u64; 4];
        let (mut start, mut hoverboard) = (false, false);
        map().for_each_pressed(&stick, |n| match n {
            Native::Key(vk) => hold(&mut keys, vk),
            Native::Start => start = true,
            Native::Hoverboard => hoverboard = true,
        });
        // free play: a coin before the start (its release counts)
        if start && !start_was && free_play {
            pulse(hwnd, VK_5);
        }
        if start {
            hold(&mut keys, VK_1);
        }
        for vk in 0..256u16 {
            let (now, was) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, keys_was[vk as usize / 64] >> (vk % 64) & 1 != 0);
            if now != was {
                post_key(hwnd, vk, now);
            }
        }
        for (held, k) in HELD.iter().zip(keys) {
            held.store(k, Ordering::Relaxed);
        }
        keys_was = keys;
        start_was = start;
        // the left stick: an arrow's press (not one already held)
        if let Some(vk) = stick_arrow(&stick, &mut armed)
            && keys[vk as usize / 64] >> (vk % 64) & 1 == 0
        {
            pulse(hwnd, vk);
        }
        if hoverboard && !hoverboard_was {
            double_tap(hwnd);
        }
        hoverboard_was = hoverboard;
    }
}

/// The lamps' Arduino: writes accepted, nothing answered.
fn handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module == "kernel32.dll" { serial::replacement(name) } else { None }
}

pub fn init() {
    // Mono's `SerialPort` (its `DllImport`s): no module's imports to hook
    serial::install_in(PORT, handler, &[]);
    mono_imports::hook(resolve);
    // loaded at the start (`payload_loader`): Unity's messages once its player is loaded
    std::thread::spawn(|| {
        let unity = loop {
            let m = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
            if m != 0 {
                break m;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        wal_payload_common::input_filter::init(unity, key_held);
        own_mouse(unity);
        std::thread::spawn(input_thread);
        log!("subway: keys and taps from the virtual sticks, the lamps' Arduino on {PORT}");
    });
}
