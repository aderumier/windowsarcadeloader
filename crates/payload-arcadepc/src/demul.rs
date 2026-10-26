//! The DemulShooter plugins' TCP inputs: Unity arcade games whose dumps carry one of
//! Argonlefou's BepInEx plugins (`<Game>_BepInEx_DemulShooter_Plugin.dll`: Quick Shot Arcade /
//! Top Gun, Happy Water Sprite II, the other SpiderZSoft games) in place of the cabinet's I/O
//! board and camera guns.
//!
//! Such a plugin listens on `127.0.0.1:<port>` (33610) for DemulShooter, its client. Each read
//! is one input packet (37 bytes): per player (4) the aim `x`, `y` (`f32`, the game window's
//! client pixels, `y` from the bottom as Unity's `Input.mousePosition`), then bit masks of the
//! players' trigger, reload, action and weapon change, then flags (bit 0 hide the crosshairs,
//! bit 7 inputs from the packets instead of the mouse). The plugin answers with output packets
//! (`u32` length, header 2, the game's lamps and recoil): read and dropped.
//!
//! `WAL_DEMUL_PORT=<port>` (the game profile): the payload is that client, from the virtual
//! sticks. The gun position (`lx`, `ly` over the screen) is mapped onto the game window. A shot
//! off the window, or with the gun off the screen (`offscreen`), is a reload: the reload bit,
//! aimed at the window's top left corner (the coordinates Top Gun reloads on).
//!
//! The plugins read the coin and the operator buttons from the keyboard (Unity's
//! `Input.GetKey`, each its own keys): natives `key_<name>` (`key_5`, `key_0`, `key_up`,
//! `key_return`...), mapped by the profile (`native_map`), are posted to the game window as key
//! messages (`input_filter::post_key`), every player's alike. Natives `trigger reload action
//! change offscreen` are each player's.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, input, input_filter, log};
use wal_protocol::{Axis, MAX_PLAYERS};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClientRect, GetSystemMetrics, GetWindowThreadProcessId, IsWindowVisible, SM_CXSCREEN, SM_CYSCREEN,
};

/// Players of an input packet.
const PLAYERS: usize = 4;
const PACKET: usize = 37;
/// Packets sent at least this often (a plugin restarted, a window moved).
const REFRESH: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Action,
    Change,
    Offscreen,
    /// A keyboard key, its virtual key.
    Key(u16),
}

/// Key names of the `key_<name>` natives (the digits and letters are their own names).
const KEY_NAMES: &[(&str, u16)] = &[
    ("return", 0x0D),
    ("enter", 0x0D),
    ("back", 0x08),
    ("backspace", 0x08),
    ("tab", 0x09),
    ("shift", 0x10),
    ("lshift", 0xA0),
    ("rshift", 0xA1),
    ("ctrl", 0x11),
    ("escape", 0x1B),
    ("space", 0x20),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("insert", 0x2D),
    ("delete", 0x2E),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pagedown", 0x22),
];

const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("b2", "change"), ("b3", "reload"), ("b6", "offscreen")];

fn natives() -> &'static [(&'static str, Native)] {
    static NATIVES: OnceLock<Vec<(&'static str, Native)>> = OnceLock::new();
    NATIVES.get_or_init(|| {
        let mut n = vec![
            ("trigger", Native::Trigger),
            ("reload", Native::Reload),
            ("action", Native::Action),
            ("change", Native::Change),
            ("offscreen", Native::Offscreen),
        ];
        let named = KEY_NAMES.iter().map(|&(name, vk)| (name.to_string(), vk));
        let digits = (b'0'..=b'9').chain(b'a'..=b'z').map(|c| ((c as char).to_string(), c.to_ascii_uppercase() as u16));
        let functions = (1..=12u16).map(|f| (format!("f{f}"), 0x6F + f));
        for (name, vk) in named.chain(digits).chain(functions) {
            n.push((Box::leak(format!("key_{name}").into_boxed_str()), Native::Key(vk)));
        }
        n
    })
}

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(natives(), DEFAULTS))
}

fn pressed(player: usize) -> Vec<Native> {
    let mut natives = Vec::new();
    map().for_each_pressed(&input(player), |n| natives.push(n));
    natives
}

/// The keys held, a bit per virtual key (256).
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Whether the game sees virtual key `vk` held (the posted keys, the filtered keyboard's only
/// ones).
pub(crate) fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

/// The game's window: a visible top-level Unity window of this process (0: none yet).
static WINDOW: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn find_window() -> HWND {
    let current = WINDOW.load(Ordering::Relaxed) as HWND;
    if !current.is_null() && unsafe { IsWindowVisible(current) } != 0 {
        return current;
    }
    let pid = unsafe { GetCurrentProcessId() };
    // Unity's window, Unreal's
    for class in [windows_sys::w!("UnityWndClass"), windows_sys::w!("UnrealWindow")] {
    let mut hwnd: HWND = std::ptr::null_mut();
    loop {
        hwnd = unsafe { FindWindowExW(std::ptr::null_mut(), hwnd, class, std::ptr::null()) };
        if hwnd.is_null() {
            break;
        }
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            log!("demul: game window {hwnd:?}");
            WINDOW.store(hwnd as usize, Ordering::Relaxed);
            return hwnd;
        }
    }
    }
    std::ptr::null_mut()
}

/// The game window's client area on the screen: left, top, width, height.
pub(crate) fn client_area(hwnd: HWND) -> Option<(f32, f32, f32, f32)> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut origin = POINT { x: 0, y: 0 };
    if hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut r) } == 0 || unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
        return None;
    }
    (r.right > 0 && r.bottom > 0).then(|| (origin.x as f32, origin.y as f32, r.right as f32, r.bottom as f32))
}

/// The gun position of `stick` (`lx`, `ly`) in screen pixels, for a `screen` size.
pub(crate) fn on_screen(stick: &wal_protocol::StickState, screen: (f32, f32)) -> (f32, f32) {
    let x = (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0 * screen.0;
    let y = (stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0 * screen.1;
    (x, y)
}

/// Where player 1's gun points in the game's picture (fractions from its top left), on it.
pub(crate) fn gun_in_picture() -> Option<(f32, f32)> {
    gun_of_player_in_picture(0)
}

/// Where `player`'s gun points in the game's picture (fractions from its top left), on it.
pub(crate) fn gun_of_player_in_picture(player: usize) -> Option<(f32, f32)> {
    let (sx, sy) = on_screen(&input(player), screen_size());
    let (x, y) = match desktop::size() {
        Some((w, h)) => {
            let (x, y) = desktop::from_screen(sx as f64, sy as f64);
            (x as f32 / w as f32, y as f32 / h as f32)
        }
        None => {
            let (left, top, w, h) = client_area(find_window())?;
            ((sx - left) / w, (sy - top) / h)
        }
    };
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

/// The screen's size in pixels.
pub(crate) fn screen_size() -> (f32, f32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) }
}

/// The input packet of the players' sticks, for a game window `area`.
fn packet(area: Option<(f32, f32, f32, f32)>, crosshairs: bool) -> [u8; PACKET] {
    let mut p = [0u8; PACKET];
    let screen = screen_size();
    for player in 0..PLAYERS.min(MAX_PLAYERS) {
        let stick = input(player);
        let natives = pressed(player);
        let on = |n| natives.contains(&n);
        // the gun position on the screen, then in the window (y from the bottom)
        let (sx, sy) = on_screen(&stick, screen);
        // a game on a desktop of its own (`WAL_DESKTOP_SIZE`): full screen on it, its pixels
        let (sx, sy, area) = match desktop::size() {
            Some((w, h)) => {
                let (x, y) = desktop::from_screen(sx as f64, sy as f64);
                (x as f32, y as f32, Some((0.0, 0.0, w as f32, h as f32)))
            }
            None => (sx, sy, area),
        };
        let (x, y, inside) = match area {
            Some((left, top, w, h)) => {
                let (x, y) = (sx - left, sy - top);
                let inside = (0.0..w).contains(&x) && (0.0..h).contains(&y);
                (x.clamp(0.0, w - 1.0), h - y.clamp(0.0, h - 1.0), inside)
            }
            None => (sx, screen.1 - sy, true),
        };
        let offscreen = on(Native::Offscreen) || !inside;
        let (x, y) = match area {
            // the window's top left corner, as Unity counts it
            Some((_, _, _, h)) if offscreen => (0.0, h),
            _ => (x, y),
        };
        p[8 * player..8 * player + 4].copy_from_slice(&x.to_le_bytes());
        p[8 * player + 4..8 * player + 8].copy_from_slice(&y.to_le_bytes());
        let trigger = on(Native::Trigger);
        let bit = 1u8 << player;
        // off the screen: the reload, and the trigger at the corner (Top Gun has no reload
        // input: its reload is a shot at (0, 0))
        if trigger {
            p[32] |= bit;
        }
        if on(Native::Reload) || (trigger && offscreen) {
            p[33] |= bit;
        }
        if on(Native::Action) {
            p[34] |= bit;
        }
        if on(Native::Change) {
            p[35] |= bit;
        }
    }
    p[36] = if crosshairs { 0 } else { 1 } | 0x80;
    p
}

/// The keys every player's natives hold.
fn keys_held() -> [u64; 4] {
    let mut held = [0u64; 4];
    for player in 0..MAX_PLAYERS {
        for n in pressed(player) {
            if let Native::Key(vk) = n {
                held[vk as usize / 64] |= 1 << (vk % 64);
            }
        }
    }
    held
}

/// Posts the keys pressed and released since `previous` to the game window.
fn update_keys(hwnd: HWND, previous: &mut [u64; 4]) {
    let held = keys_held();
    if hwnd.is_null() || held == *previous {
        return;
    }
    for vk in 0..256u16 {
        let (word, bit) = (vk as usize / 64, 1u64 << (vk % 64));
        let (was, is) = (previous[word] & bit != 0, held[word] & bit != 0);
        if was != is {
            // held first, so the game's key state polling agrees with the message
            if is {
                HELD[word].fetch_or(bit, Ordering::Relaxed);
            }
            input_filter::post_key(hwnd, vk, is);
            if !is {
                HELD[word].fetch_and(!bit, Ordering::Relaxed);
            }
            log!("demul: key {vk:#04x} {}", if is { "down" } else { "up" });
        }
    }
    *previous = held;
}

/// Starts the client of the plugin on `port`.
pub fn start(port: u16) {
    log!("demul: DemulShooter plugin client, port {port}");
    std::thread::spawn(move || {
        let crosshairs = wal_payload_common::crosshairs(true);
        let mut keys = [0u64; 4];
        let mut stream: Option<TcpStream> = None;
        let mut last = ([0u8; PACKET], Instant::now());
        let mut next_connect = Instant::now();
        let mut last_area = None;
        loop {
            std::thread::sleep(Duration::from_millis(4));
            let hwnd = find_window();
            update_keys(hwnd, &mut keys);
            if stream.is_none() && Instant::now() >= next_connect {
                let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
                match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
                    Ok(s) => {
                        log!("demul: connected to the plugin");
                        let _ = s.set_nodelay(true);
                        let _ = s.set_nonblocking(true);
                        stream = Some(s);
                        last.1 = Instant::now() - REFRESH;
                    }
                    Err(_) => next_connect = Instant::now() + Duration::from_millis(500),
                }
            }
            let Some(s) = stream.as_mut() else { continue };
            // the plugin's outputs, dropped (its socket closed: reconnect)
            let mut buf = [0u8; 512];
            let closed = loop {
                match s.read(&mut buf) {
                    Ok(0) => break true,
                    Ok(_) => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break false,
                    Err(_) => break true,
                }
            };
            let area = client_area(hwnd);
            if area != last_area {
                log!("demul: game window area {area:?}");
                last_area = area;
            }
            let p = packet(area, crosshairs);
            if !closed && (p != last.0 || last.1.elapsed() >= REFRESH) {
                // one packet per plugin read: a write the socket takes at once, whole
                let closed = s.set_nonblocking(false).and_then(|_| s.write_all(&p)).and_then(|_| s.set_nonblocking(true)).is_err();
                if !closed {
                    last = (p, Instant::now());
                    continue;
                }
            } else if !closed {
                continue;
            }
            log!("demul: plugin disconnected");
            stream = None;
            next_connect = Instant::now() + Duration::from_millis(500);
        }
    });
}
