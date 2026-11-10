//! Wii static recompilations' Windows builds (SDL2): `WAL_ARCADEPC_BOARD=wiirecomp`. The House of
//! the Dead: Overkill's (https://github.com/darkuni/house-of-the-dead-overkill-recomp).
//!
//! The runtime's Wii Remote is the mouse (`SDL_GetMouseState`: SDL's state from the window's
//! messages): its place in the picture is the remote's pointer (off the picture: pointing off
//! the screen), left button B (the trigger), right button A, middle button the reload; Enter
//! is +, Esc / F1 the runtime's settings menu. The payload posts those messages from player 1's
//! gun and stick; the real keyboard and mouse never reach SDL (`payload-common`'s
//! `input_filter` on SDL2.dll's message loop).
//!
//! Natives: `trigger a reload plus minus settings` (the title wants A and B together: start
//! presses both).
//!
//! Two to four guns: a runtime built with its XInput guns (`RECOMP_XINPUT_GUNS=1`: Wii Remote n
//! is XInput pad n, the left stick the gun on the screen, RT = B, RB = A, LB = reload, START = A + B,
//! BACK = +) reads the payload's pads (`pads.rs`, one per player); the mouse is then not posted.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

/// `wParam`'s button flags.
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_MBUTTON: usize = 0x0010;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// Mouse buttons (`MK_` flags).
    Mouse(usize),
    /// A key: its virtual key.
    Key(u16),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Mouse(MK_LBUTTON)),
    ("a", Native::Mouse(MK_RBUTTON)),
    ("reload", Native::Mouse(MK_MBUTTON)),
    ("ab", Native::Mouse(MK_LBUTTON | MK_RBUTTON)),
    // Enter, Backspace, Esc
    ("plus", Native::Key(0x0D)),
    ("minus", Native::Key(0x08)),
    ("settings", Native::Key(0x1B)),
];

const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("b2", "reload"), ("b3", "a"), ("start", "ab"), ("coin", "plus"), ("test", "settings")];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keys held (bit n: virtual key n), for SDL's key state polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn input_thread() {
    let (mut keys_was, mut buttons_was, mut last) = ([0u64; 4], 0usize, (i32::MIN, i32::MIN));
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let mut keys = [0u64; 4];
        let mut buttons = 0usize;
        map().for_each_pressed(&input(0), |n| match n {
            Native::Mouse(b) => buttons |= b,
            Native::Key(vk) => keys[vk as usize / 64] |= 1 << (vk % 64),
        });
        for vk in 0..256u16 {
            let (now, was) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, keys_was[vk as usize / 64] >> (vk % 64) & 1 != 0);
            if now != was {
                if now {
                    HELD[vk as usize / 64].fetch_or(1 << (vk % 64), Ordering::Relaxed);
                }
                post_key(hwnd, vk, now);
                if !now {
                    HELD[vk as usize / 64].fetch_and(!(1 << (vk % 64)), Ordering::Relaxed);
                }
            }
        }
        keys_was = keys;
        // the gun: the mouse on the window's picture; off it, just outside the window (the remote
        // pointing off the screen)
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(hwnd, &mut r) };
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32),
            None => (-8, r.bottom / 2),
        };
        if at != last {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        for (flag, down, up) in [(MK_LBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP), (MK_RBUTTON, WM_RBUTTONDOWN, WM_RBUTTONUP), (MK_MBUTTON, WM_MBUTTONDOWN, WM_MBUTTONUP)] {
            if buttons & flag != buttons_was & flag {
                post_mouse(hwnd, if buttons & flag != 0 { down } else { up }, last.0, last.1, buttons);
            }
        }
        buttons_was = buttons;
    }
}

pub fn init() {
    let sdl = unsafe { GetModuleHandleW(windows_sys::w!("SDL2.dll")) } as usize;
    if std::env::var("RECOMP_XINPUT_GUNS").is_ok_and(|v| v.trim() == "1") {
        wal_payload_common::input_filter::init(sdl, crate::pads::key_held);
        own_mouse(sdl);
        log!("wiirecomp: the Wii Remotes are the players' XInput pads (SDL2.dll {sdl:#x})");
        return;
    }
    crate::pads::disconnect();
    wal_payload_common::input_filter::init(sdl, key_held);
    own_mouse(sdl);
    std::thread::spawn(input_thread);
    log!("wiirecomp: the Wii Remote's mouse and keys from player 1's gun and stick (SDL2.dll {sdl:#x})");
}
