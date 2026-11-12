//! Fix-It Felix Jr. (the Wreck-It Ralph promotional arcade game: `FixitFelixJr.exe`, the
//! "FOCAL" test shell, OpenGL, 32-bit): `WAL_ARCADEPC_BOARD=felix`.
//!
//! The game reads only window messages: its window procedure (0x414040) turns `WM_KEYDOWN` /
//! `WM_KEYUP` into bits of five input words (0x4117a0): the system word (`1` / `2` the players'
//! starts, `F1`..`F4`, `F8`, Backspace) and player 1's (the arrows, Shift / `X` the hammer,
//! Space / `Z` the jump, `C` the coin...), players 2 and 3 for the network play. Its other keys are
//! debugging ones (Escape leaves full screen, `F5`..`F7`, `F9`, `F11` writes `Controls.log`,
//! Print Screen), the mouse its left button (both buttons) and pointer. The payload posts
//! player 1's keys from the virtual sticks (`payload-common`'s `input_filter`: the real keyboard
//! and mouse never reach it): the arrows, `X` (hammer), `Z` (jump), `C` (coin), `1` / `2`
//! (start).
//!
//! **Window.** The shell's window is `zoom` (0x418054, a float: its Zoom menu's 33 %, 50 %,
//! 100 % or 200 %) times 480x640, and each frame it sizes its window back to that unless its
//! client area divided by the zoom is 480x640; its picture (OpenGL) is drawn that size from the
//! client area's bottom left. The payload sets the zoom (and the `-zoom` default, 0x41803c,
//! which the window's creation copies when it is one of the menu's) to the height of the
//! window the profile gives it (`WAL_WINDOW_SIZE=aspect`, 3:4): the picture fills the screen's
//! height.
//!
//! **Free play** (the common `WAL_FREEPLAY`): the `-user` argument (0x420288) holding `free`
//! keeps the credits at 2 (each frame, 0x40b210). The payload points it at `free` before the
//! command line is read (a `-user` argument still wins).
//!
//! Natives: `up`, `down`, `left`, `right`, `fix` (`X`), `jump` (`Z`), `coin`, `start`, `start2`
//! (player 1; player 2 plays in turn on the same controls).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

/// The game's main window handle.
const HWND_VAR: usize = 0x420324;
/// The zoom in use, and the `-zoom` argument's (default 1.0).
const ZOOM: usize = 0x418054;
const ZOOM_ARG: usize = 0x41803c;
/// The `-user` argument (a `malloc`ed string, null without one).
const USER: usize = 0x420288;
/// The shell's picture at zoom 1 (half its 100 % window).
const PICTURE: (f64, f64) = (240.0, 320.0);

const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_1: u16 = 0x31;
const VK_2: u16 = 0x32;
const VK_C: u16 = 0x43;
const VK_X: u16 = 0x58;
const VK_Z: u16 = 0x5A;

const NATIVES: &[(&str, u16)] = &[
    ("up", VK_UP),
    ("down", VK_DOWN),
    ("left", VK_LEFT),
    ("right", VK_RIGHT),
    ("fix", VK_X),
    ("jump", VK_Z),
    ("coin", VK_C),
    ("start", VK_1),
    ("start2", VK_2),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "fix"),
    ("b2", "jump"),
    ("coin", "coin"),
    ("start", "start"),
    ("b6", "start2"),
];

fn map() -> &'static ButtonMap<u16> {
    static MAP: OnceLock<ButtonMap<u16>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keys held (bit n: virtual key n), for the game's key state polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn exe(offset: usize) -> usize {
    unsafe { GetModuleHandleW(std::ptr::null()) as usize + offset - 0x400000 }
}

/// The zoom whose window is `w` x `h` for the shell's size check: (its client area / the zoom)
/// truncated, halved, is 240x320.
fn zoom_for(w: i32, h: i32) -> f32 {
    let fits = |z: f32| {
        let (x, y) = ((w as f64 / z as f64) as i32 / 2, (h as f64 / z as f64) as i32 / 2);
        (x, y) == (PICTURE.0 as i32, PICTURE.1 as i32)
    };
    let mut z = (h as f64 / (2.0 * PICTURE.1)) as f32;
    for _ in 0..64 {
        if fits(z) {
            break;
        }
        // a quotient under 480x640: a smaller zoom
        let under = (h as f64 / z as f64) < 2.0 * PICTURE.1 || (w as f64 / z as f64) < 2.0 * PICTURE.0;
        z = f32::from_bits(if under { z.to_bits() - 1 } else { z.to_bits() + 1 });
    }
    z
}

fn input_thread() {
    let mut was = [0u64; 4];
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = unsafe { *(exe(HWND_VAR) as *const usize) } as HWND;
        if hwnd.is_null() {
            continue;
        }
        let stick = input(0);
        let mut keys = [0u64; 4];
        map().for_each_pressed(&stick, |vk| keys[vk as usize / 64] |= 1 << (vk % 64));
        for (held, k) in HELD.iter().zip(keys) {
            held.store(k, Ordering::Relaxed);
        }
        for vk in 0..256u16 {
            let (now, before) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, was[vk as usize / 64] >> (vk % 64) & 1 != 0);
            if now != before {
                post_key(hwnd, vk, now);
            }
        }
        was = keys;
    }
}

pub fn init() {
    let exe_module = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe_module, key_held);
    own_mouse(exe_module);
    // the window WAL_WINDOW_SIZE=aspect gives it: 3:4 on the screen
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let (w, h) = if sw * 4 > sh * 3 { (sh * 3 / 4, sh) } else { (sw, sw * 4 / 3) };
    let zoom = zoom_for(w, h);
    unsafe {
        *(exe(ZOOM) as *mut f32) = zoom;
        *(exe(ZOOM_ARG) as *mut f32) = zoom;
    }
    let free = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(false);
    if free {
        static FREE: &[u8] = b"free\0";
        unsafe { *(exe(USER) as *mut usize) = FREE.as_ptr() as usize };
    }
    std::thread::spawn(input_thread);
    log!("felix: keys from the virtual sticks, zoom {zoom} ({w}x{h}), free play {free}");
}
