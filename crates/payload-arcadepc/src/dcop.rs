//! DCOP (Klockworks, Unity 2020.3, Mono, 64-bit): `WAL_ARCADEPC_BOARD=dcop`.
//!
//! The game plays with the mouse: `GunScript` shoots at `Input.mousePosition` on `Fire1` (the
//! left button; the machine gun while it is held), reloads on `Fire2` (the right button), and
//! any key or button starts a game in the attract mode (`AttractMode`: `Input.anyKeyDown`) or
//! continues one (`Continue`: `Input.anyKey`): there are no coins. The cabinet's Arduino
//! (Uduino: the lamps, the recoil, the gun's light) only takes outputs; the dump's DemulShooter
//! BepInEx plugin (`DCOP_BepInEx_DemulShooter_Plugin.dll`) keeps Uduino from opening any serial
//! port, saves to the dump's `NVRAM` folder, makes the pause menu (`PauseGame`, the operator's
//! menu: volume, exit) open on `0` instead of `Escape` (its own exit key) and plays at the
//! resolution of its `.ini`.
//!
//! The payload plays the mouse with the virtual sticks: the gun is the mouse (its place on the
//! game window's picture, Unity's `GetCursorPos` answered with it), the trigger its left button,
//! the reload its right one (also a shot off the picture, or with the gun off the screen), the
//! keys posted (`payload-common`'s `input_filter`: the real keyboard and mouse never reach the
//! game, moves included).
//!
//! The game's own crosshair is the system cursor (`SetMouseCursor`: `Cursor.SetCursor`), drawn
//! where the real pointer is: hidden by the plugin, the payload being its client on
//! `WAL_DEMUL_PORT` (its input packet is two bytes here: `EnableInputsHack`, `HideCrosshairs`,
//! the fields sorted by name); the common crosshair option (`WAL_CROSSHAIR`, `pointer_d3d11`)
//! draws the gun's.
//!
//! Natives: `trigger reload offscreen` (player 1), `key_<name>` keys (as `demul.rs`: `key_0`
//! the pause menu, `key_up` / `key_down` / `key_return` its navigation, `key_5` a start key).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, gun, input, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

/// `wParam`'s button flags.
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Offscreen,
    /// A keyboard key: its virtual key.
    Key(u16),
}

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "reload"),
    ("b6", "offscreen"),
    ("start", "key_return"),
    ("coin", "key_5"),
    ("test", "key_0"),
    ("up", "key_up"),
    ("down", "key_down"),
];

fn natives() -> &'static [(&'static str, Native)] {
    static NATIVES: OnceLock<Vec<(&'static str, Native)>> = OnceLock::new();
    NATIVES.get_or_init(|| {
        let mut n = vec![("trigger", Native::Trigger), ("reload", Native::Reload), ("offscreen", Native::Offscreen)];
        let named = [("return", 0x0D), ("enter", 0x0D), ("space", 0x20), ("up", 0x26), ("down", 0x28), ("left", 0x25), ("right", 0x27)];
        let named = named.into_iter().map(|(name, vk)| (name.to_string(), vk));
        let chars = (b'0'..=b'9').chain(b'a'..=b'z').map(|c| ((c as char).to_string(), c.to_ascii_uppercase() as u16));
        for (name, vk) in named.chain(chars) {
            n.push((Box::leak(format!("key_{name}").into_boxed_str()), Native::Key(vk)));
        }
        n
    })
}

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(natives(), DEFAULTS))
}

/// The keys held (bit n: virtual key n), for the game's key state polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn input_thread() {
    let (mut keys_was, mut buttons_was, mut last) = ([0u64; 4], 0usize, (i32::MIN, i32::MIN));
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let mut keys = [0u64; 4];
        let (mut trigger, mut reload, mut offscreen) = (false, false, false);
        map().for_each_pressed(&input(0), |n| match n {
            Native::Trigger => trigger = true,
            Native::Reload => reload = true,
            Native::Offscreen => offscreen = true,
            Native::Key(vk) => keys[vk as usize / 64] |= 1 << (vk % 64),
        });
        for vk in 0..256u16 {
            let (now, was) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, keys_was[vk as usize / 64] >> (vk % 64) & 1 != 0);
            if now != was {
                // held first, so the game's key state polling agrees with the message
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
        // the gun: the mouse on the window's picture, in the game's own pixels (Unity takes the
        // cursor's client coordinates as its screen's pixels)
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        match desktop::size() {
            Some((w, h)) => (r.right, r.bottom) = (w, h),
            None => {
                unsafe { GetClientRect(hwnd, &mut r) };
            }
        }
        let picture = gun::gun_of_player_in_picture(0);
        let at = match picture {
            Some((x, y)) => ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32),
            None => last,
        };
        // off the picture (or the screen): the trigger reloads
        let away = picture.is_none() || offscreen;
        let buttons = if trigger && !away { MK_LBUTTON } else { 0 } | if reload || (trigger && away) { MK_RBUTTON } else { 0 };
        if at != last && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        let (x, y) = if last.0 == i32::MIN { (0, 0) } else { last };
        for (flag, down, up) in [(MK_LBUTTON, WM_LBUTTONDOWN, WM_LBUTTONUP), (MK_RBUTTON, WM_RBUTTONDOWN, WM_RBUTTONUP)] {
            if buttons & flag != buttons_was & flag {
                post_mouse(hwnd, if buttons & flag != 0 { down } else { up }, x, y, buttons);
            }
        }
        buttons_was = buttons;
    }
}

/// The plugin's client: its input packet hides the game's cursor (the real pointer's place).
fn plugin_thread(port: u16) {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) else { continue };
        // EnableInputsHack 0 (its own mouse reads, unused by the game), HideCrosshairs 1
        if s.write_all(&[0, 1]).is_err() {
            continue;
        }
        log!("dcop: the plugin hides the game's cursor");
        // its outputs (lamps, recoil), dropped, until it closes the connection
        let mut buf = [0u8; 512];
        while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
        log!("dcop: the plugin closed the connection");
        return;
    }
}

pub fn init(unity: usize) {
    wal_payload_common::input_filter::init(unity, key_held);
    own_mouse(unity);
    std::thread::spawn(input_thread);
    if let Some(port) = std::env::var("WAL_DEMUL_PORT").ok().and_then(|p| p.trim().parse().ok()) {
        std::thread::spawn(move || plugin_thread(port));
    }
    log!("dcop: the mouse played by the gun");
}
