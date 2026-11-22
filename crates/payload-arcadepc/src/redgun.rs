//! Red Gun (a Unity 4 mobile shooting game, `RedGun.exe`, Mono, 32-bit; the dump is a PC repack
//! for light guns): `WAL_ARCADEPC_BOARD=redgun`, x86 build.
//!
//! The repack's `Assembly-CSharp.dll` reads every touch through its `RGDesktopInput` class
//! (`RGDesktopInput.dll`): the pointer (`MousePosition`: `GetCursorPos`, `ScreenToClient` on
//! `GetActiveWindow`'s client area), the mouse buttons and the arrows (`GetAsyncKeyState`, only
//! while `GetActiveWindow` is `GetForegroundWindow`): the left button shoots and taps the menus,
//! the right one goes to cover and out again (the cover reloads), an arrow steps out of cover
//! that way (the touch game's swipes). No original assembly exists without that layer: its
//! `user32.dll` `DllImport`s are the payload's (`mono_imports`), answering the launcher's gun
//! and buttons alone, whatever the window's focus; the real keyboard and mouse never reach the
//! game (`input_filter`: Unity's messages and key polls, the repack's Sinden border keys).
//!
//! The gun is the cursor on the window's picture (its client area: the repack stretches the
//! game's 3:2 cameras over it, or pillarboxes them with `-rg-original-aspect`), kept at its last
//! place off it; a shot off the screen goes to cover instead (the gun's reload). The game draws
//! no pointer of its own: the common crosshair (`WAL_CROSSHAIR`) over its Direct3D 9 picture.
//!
//! Natives: `trigger` (the left button), `cover` (the right button), `offscreen` (the trigger
//! goes to cover), `up` `down` `left` `right` (the arrows: out of cover).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Cover,
    Offscreen,
    /// An arrow: its virtual key.
    Key(i32),
}

const VK_LBUTTON: i32 = 0x01;
const VK_RBUTTON: i32 = 0x02;

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("cover", Native::Cover),
    ("offscreen", Native::Offscreen),
    ("left", Native::Key(0x25)),
    ("up", Native::Key(0x26)),
    ("right", Native::Key(0x27)),
    ("down", Native::Key(0x28)),
];

/// The gun's second button goes to cover, a shot off the screen too; the pad's directions step
/// out of cover.
const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "cover"),
    ("b6", "offscreen"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// Whether the game sees virtual key `vk` down: player 1's buttons now.
fn key_down(vk: i32) -> bool {
    let (mut trigger, mut cover, mut offscreen, mut arrow) = (false, false, false, false);
    map().for_each_pressed(&input(0), |n| match n {
        Native::Trigger => trigger = true,
        Native::Cover => cover = true,
        Native::Offscreen => offscreen = true,
        Native::Key(k) => arrow |= k == vk,
    });
    // off the screen the trigger goes to cover
    if offscreen && trigger {
        (trigger, cover) = (false, true);
    }
    match vk {
        VK_LBUTTON => trigger,
        VK_RBUTTON => cover,
        _ => arrow,
    }
}

unsafe extern "system" fn get_async_key_state(vk: i32) -> i16 {
    if key_down(vk) { i16::MIN } else { 0 }
}

/// `GetActiveWindow`, `GetForegroundWindow`: the game's window (focused for the repack's polls).
unsafe extern "system" fn game_window() -> HWND {
    gun::find_window()
}

/// The last cursor answered (x << 32 | y, client pixels), kept while the gun is off the screen.
static CURSOR: AtomicU64 = AtomicU64::new(0);

/// `GetCursorPos`: the gun on the window's picture, in client pixels (`ScreenToClient` keeps
/// them).
unsafe extern "system" fn get_cursor_pos(point: *mut POINT) -> i32 {
    if point.is_null() {
        return 0;
    }
    let hwnd = gun::find_window();
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if !hwnd.is_null() {
        unsafe { GetClientRect(hwnd, &mut r) };
    }
    if let Some((x, y)) = gun::gun_of_player_in_picture(0) {
        let at = ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32);
        CURSOR.store((at.0 as u32 as u64) << 32 | at.1 as u32 as u64, Ordering::Relaxed);
    }
    let at = CURSOR.load(Ordering::Relaxed);
    unsafe { *point = POINT { x: (at >> 32) as i32, y: at as i32 } };
    1
}

unsafe extern "system" fn screen_to_client(_hwnd: HWND, _point: *mut POINT) -> i32 {
    1
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module != "user32.dll" {
        return None;
    }
    let f = match name {
        "GetAsyncKeyState" => get_async_key_state as *const () as usize,
        "GetActiveWindow" | "GetForegroundWindow" => game_window as *const () as usize,
        "GetCursorPos" => get_cursor_pos as *const () as usize,
        "ScreenToClient" => screen_to_client as *const () as usize,
        _ => return None,
    };
    Some(f)
}

pub fn init() {
    mono_imports::hook(resolve);
    // Unity 4: the player is the executable
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, |_| false);
    wal_payload_common::crosshair::init();
    log!("redgun: the repack's mouse and arrows played by the gun");
}
