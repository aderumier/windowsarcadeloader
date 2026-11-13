//! Cut The Rope (the ZeptoLab redemption conversion: `CTR MonoGame Windows.exe`, a .NET
//! MonoGame game on SDL/OpenGL, portrait 1080x1920, 32-bit): `WAL_ARCADEPC_BOARD=cuttherope`.
//!
//! The cabinet's I/O is a serial board (coins, the ticket dispenser) its `IOBoard` opens on
//! `/dev/ttyACM0`, and the touch screen a Baanto HID panel read from `/dev/hidrawN`: both only
//! on the real Linux cabinet (`Util.OnDevice`, true when `Environment.OSVersion.Platform` is
//! Unix). Run as a Windows program under Wine the platform is not Unix, so the game opens
//! neither device and takes its input the desktop way: the MonoGame `Mouse` (the Windows cursor,
//! its left button) for the touches, and the MonoGame `Keyboard` for the operator keys. Its
//! `CTRGame.Update` reads, with the serial board absent:
//!
//! * **Space** inserts a credit (the board's `Coin1`), **D** opens the operator/audit menu
//!   (the board's `Diag`), **E** the test menu, **Escape** quits (the launcher's own exit, not
//!   sent here). The audit menu itself is touch-driven (its on-screen buttons), with **Q** the
//!   diag button that leaves its touch test.
//!
//! The payload plays that mouse and those keys from the virtual sticks (`payload-common`'s
//! `input_filter`: the real keyboard and mouse never reach the game): the gun (or mouse) is the
//! cursor on the game's picture, the trigger its left button (a message and a raw report), and
//! `coin` / `test` / `service` post Space / D / E.
//!
//! Natives: `touch` (player 1, the cursor's left button), `coin`, `test` (operator menu),
//! `service`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse, post_raw_buttons};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, gun, input, log};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;
const RI_MOUSE_LEFT_BUTTON_DOWN: u16 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_UP: u16 = 0x0002;

/// The operator keys (XNA `Keys` values, the same as the Windows virtual keys).
const VK_SPACE: u16 = 0x20;
const VK_D: u16 = 0x44;
const VK_E: u16 = 0x45;

/// Virtual input -> operator key.
const NATIVES: &[(&str, u16)] = &[("coin", VK_SPACE), ("test", VK_D), ("service", VK_E)];
const DEFAULTS: &[(&str, &str)] = &[("coin", "coin"), ("test", "test"), ("service", "service")];

fn keymap() -> &'static ButtonMap<u16> {
    static MAP: OnceLock<ButtonMap<u16>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The touch (the cursor's left button): its own map, so it is not one of the operator keys.
#[derive(Clone, Copy, PartialEq)]
enum Touch {
    Touch,
}
const TOUCH_NATIVES: &[(&str, Touch)] = &[("touch", Touch::Touch)];
const TOUCH_DEFAULTS: &[(&str, &str)] = &[("b1", "touch"), ("start", "touch")];

fn touchmap() -> &'static ButtonMap<Touch> {
    static MAP: OnceLock<ButtonMap<Touch>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(TOUCH_NATIVES, TOUCH_DEFAULTS))
}

/// The operator keys held (bit n: virtual key n), for the game's `Keyboard` polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
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

fn input_thread() {
    let mut key_was = [0u64; 4];
    let (mut touch_was, mut last) = (false, (i32::MIN, i32::MIN));
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let stick = input(0);

        // the operator keys
        let mut keys = [0u64; 4];
        keymap().for_each_pressed(&stick, |vk| keys[vk as usize / 64] |= 1 << (vk % 64));
        for (held, k) in HELD.iter().zip(keys) {
            held.store(k, Ordering::Relaxed);
        }
        for vk in 0..256u16 {
            let (now, before) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, key_was[vk as usize / 64] >> (vk % 64) & 1 != 0);
            if now != before {
                post_key(hwnd, vk, now);
            }
        }
        key_was = keys;

        // the touch: the gun (or mouse) is the cursor, the trigger its left button
        let mut touch = false;
        touchmap().for_each_pressed(&stick, |t| if t == Touch::Touch { touch = true });
        let (w, h) = desktop::size().unwrap_or_else(|| {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut r) };
            (r.right, r.bottom)
        });
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * w as f32) as i32, (y * h as f32) as i32),
            None => last,
        };
        let buttons = if touch { MK_LBUTTON } else { 0 };
        // again every half second: a cursor that did not move still reaches a new screen
        tick = (tick + 1) % 64;
        if (at != last || tick == 0) && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if touch != touch_was && last.0 != i32::MIN {
            button(hwnd, touch, last);
        }
        touch_was = touch;
    }
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, key_held);
    own_mouse(exe);
    std::thread::spawn(input_thread);
    log!("cuttherope: the mouse and the operator keys played by the sticks");
}
