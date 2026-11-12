//! Monsterous (`PX.exe`, SDL 1.2, 32-bit, 1280x768): `WAL_ARCADEPC_BOARD=monsterous`, x86
//! build.
//!
//! A coin pusher's screen game: the cabinet fires the player's coins at ten targets in front of
//! the screen (`Setup\PinPos.txt`'s pins, `0x412010` their places on the screen: three rows),
//! a monster on the hit target is caught; the cabinet's "Logic" board (serial, `IO`) runs the
//! coin mechs, hoppers, lift, slides and the targets' sensors. The profile runs the game's own
//! desktop mode (`NOIO`: the board simulated by the game, its hoppers' coins dispensed in
//! software, `WINDOW`, `NOCURSOR`), where its debug keys stand in for the board:
//!
//! - **Targets.** A digit key (`1`-`9`, `0` the tenth target, `0x402330`) with credits fires a
//!   coin (`0x411ac0`) that hits its target. The payload keeps an aimed target (the stick or
//!   d-pad: left / right along its row, up / down to the nearest of the next row; the bottom
//!   middle one at start), drawn as a ring on the picture (`SDL_Flip`'s screen surface), and
//!   the fire button posts its digit (again every 250 ms while held, as the cabinet's button)
//!   in the main and bonus screens (`0x431d58`: 0, 1). In the instructions (6-9, shown at a
//!   credit's first coin and in the attract) it is the cabinet's "fire button to skip": the
//!   screen's timer ends at once (`0x6077fc` = 1, as the board's fire switch does, `0x40c800`).
//! - **Coins.** A coin is the game's coin in (`0x40c3c0`, the coin's value `0x6078de`), the
//!   service button the same (a service credit). Free play (the common `WAL_FREEPLAY`): a coin
//!   whenever the credits (`0x6065d8`) are spent.
//! - **Operator menu.** The board's operator switches (`0x6021a8`, never written in the desktop
//!   mode): bit 0 the menu's key switch (the menu shown while it is on: the test button turns it
//!   on and off), 1 / 2 / 3 previous item, next item, select (the up, down and fire buttons,
//!   held as pressed: the menu takes their presses). The desktop mode's keys for them (`s`,
//!   the arrows) are read from the key of the frame's last event poll, which the menu's frame
//!   polls again without.
//! - **Setup alarm.** A fresh install (no saved options) shows "Setup Alarm, Please Call the
//!   Operator" until the operator menu is left once: the options' check value (`0x6078c8`,
//!   `0x2a`) is set at the first frame.
//! - **Input.** `SDL_PollEvent`: the real keyboard's and mouse's events are dropped, the
//!   payload's keys queued (`SDL_KEYDOWN` / `SDL_KEYUP`); `SDL_GetKeyState` (the volume keys)
//!   gives no key down.
//! - **Picture.** SDL's `windib` driver draws its 1280x768 surface into its window with
//!   `BitBlt`: the window, on a 1280x768 desktop fitted onto the screen (`WAL_DESKTOP_SIZE`), is
//!   larger, so `SDL.dll`'s `BitBlt` is a `StretchBlt` to the window's client area.
//! - `WAL_MONSTEROUS_TRACE=1`: the scene, level, credits, shots and targets' states logged each
//!   second.
//!
//! Natives: `up down left right fire coin service menu`.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{COLORONCOLOR, SetStretchBltMode, StretchBlt, WindowFromDC};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

type P = *mut c_void;

/// The game's addresses (image base 0x400000, no relocations).
/// `(x, y) = target(i)` (cdecl, x in eax, y in edx): target `i`'s place on the screen.
const TARGET_PLACE: usize = 0x412010;
/// `coin_in(value)` (cdecl).
const COIN_IN: usize = 0x40c3c0;
const COIN_VALUE: usize = 0x6078de;
const CREDITS: usize = 0x6065d8;
/// The board's operator switches: the menu's key switch, previous, next, select.
const OPERATOR: usize = 0x6021a8;
const MENU_KEY: u32 = 1;
const MENU_PREVIOUS: u32 = 2;
const MENU_NEXT: u32 = 4;
const MENU_SELECT: u32 = 8;
/// The operator menu shown (i16).
const MENU_ON: usize = 0x6055ae;
/// The scene shown (0 main, 1 bonus, 2-5 the bonus' instructions and result, 6-9 the
/// instructions), its timer (the next scene at that tick; 1: at once).
const SCENE: usize = 0x431d58;
const SCENE_TIMER: usize = 0x6077fc;
const SETUP_CHECK: usize = 0x6078c8;
const SETUP_OK: u16 = 0x2a;
/// The video mode's size.
const MODE_W: usize = 0x60609c;
const MODE_H: usize = 0x6060a0;

const TARGETS: usize = 10;
const FIRE_REPEAT: Duration = Duration::from_millis(250);

/// SDL 1.2 event types, key symbols.
const SDL_KEYDOWN: u8 = 2;
const SDL_KEYUP: u8 = 3;
const SDL_MOUSEMOTION: u8 = 4;
const SDL_JOYBUTTONUP: u8 = 11;
const SDL_PRESSED: u8 = 1;
const EVENT_SIZE: usize = 20;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Up,
    Down,
    Left,
    Right,
    Fire,
    Coin,
    Service,
    Menu,
}

const NATIVES: &[(&str, Native)] = &[
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
    ("fire", Native::Fire),
    ("coin", Native::Coin),
    ("service", Native::Service),
    ("menu", Native::Menu),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "fire"),
    ("start", "fire"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "menu"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

struct State {
    held: Vec<Native>,
    /// Keys not handed to the game yet: symbol, pressed.
    queue: VecDeque<(u32, bool)>,
    aim: usize,
    next_fire: Option<Instant>,
    started: bool,
    menu_key: bool,
    last_trace: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { held: Vec::new(), queue: VecDeque::new(), aim: 8, next_fire: None, started: false, menu_key: false, last_trace: None });
static TRACE: AtomicBool = AtomicBool::new(false);
static AIM: AtomicUsize = AtomicUsize::new(8);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);
static COINS: AtomicU32 = AtomicU32::new(0);

fn read_i16(addr: usize) -> i16 {
    unsafe { std::ptr::read_volatile(addr as *const i16) }
}

fn targets() -> [(i32, i32); TARGETS] {
    let place: unsafe extern "C" fn(i32) -> u64 = unsafe { std::mem::transmute(TARGET_PLACE) };
    std::array::from_fn(|i| {
        let v = unsafe { place(i as i32) };
        (v as u32 as i32, (v >> 32) as u32 as i32)
    })
}

/// The target beside `from` in a direction: along its row (left / right), or the nearest one
/// of the next row up or down.
fn step(t: &[(i32, i32); TARGETS], from: usize, dir: Native) -> usize {
    let (x, y) = t[from];
    let pick = |ok: &dyn Fn(i32, i32) -> bool, key: &dyn Fn(i32, i32) -> i64| {
        (0..TARGETS).filter(|&i| ok(t[i].0, t[i].1)).min_by_key(|&i| key(t[i].0, t[i].1)).unwrap_or(from)
    };
    let same_row = |ty: i32| (ty - y).abs() < 40;
    match dir {
        Native::Left => pick(&|tx, ty| same_row(ty) && tx < x, &|tx, _| i64::from(x - tx)),
        Native::Right => pick(&|tx, ty| same_row(ty) && tx > x, &|tx, _| i64::from(tx - x)),
        Native::Up | Native::Down => {
            let up = dir == Native::Up;
            let row = (0..TARGETS)
                .map(|i| t[i].1)
                .filter(|&ty| !same_row(ty) && (ty < y) == up)
                .min_by_key(|&ty| (ty - y).abs());
            match row {
                Some(r) => pick(&|_, ty| (ty - r).abs() < 40, &|tx, _| i64::from((tx - x).abs())),
                None => from,
            }
        }
        _ => from,
    }
}

fn press(queue: &mut VecDeque<(u32, bool)>, sym: u32) {
    queue.push_back((sym, true));
    queue.push_back((sym, false));
}

/// The aimed target's digit key.
fn digit(aim: usize) -> u32 {
    u32::from(if aim == 9 { b'0' } else { b'1' + aim as u8 })
}

fn coin_in() {
    let f: unsafe extern "C" fn(i32) = unsafe { std::mem::transmute(COIN_IN) };
    let value = i32::from(read_i16(COIN_VALUE)).max(1);
    unsafe { f(value) };
    let n = COINS.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 3 {
        log!("monsterous: coin in ({value}), credits {}", read_i16(CREDITS));
    }
}

/// The frame's inputs, on the game's thread (before its events are read).
fn update(s: &mut State) {
    if !s.started {
        s.started = true;
        let check = SETUP_CHECK as *mut u16;
        if unsafe { check.read_volatile() } != SETUP_OK {
            unsafe { check.write_volatile(SETUP_OK) };
            log!("monsterous: setup alarm cleared (no saved options)");
        }
        log!("monsterous: targets {:?}", targets());
    }
    let mut now = Vec::new();
    map().for_each_pressed(&input(0), |n| {
        if !now.contains(&n) {
            now.push(n);
        }
    });
    let pressed: Vec<Native> = now.iter().copied().filter(|n| !s.held.contains(n)).collect();
    s.held = now;
    let menu = read_i16(MENU_ON) != 0;
    let held = |n| s.held.contains(&n);
    let mut switches = if s.menu_key { MENU_KEY } else { 0 };
    if menu {
        for (n, bit) in [(Native::Up, MENU_PREVIOUS), (Native::Down, MENU_NEXT), (Native::Fire, MENU_SELECT)] {
            if held(n) {
                switches |= bit;
            }
        }
    }
    let scene = read_i16(SCENE);
    if TRACE.load(Ordering::Relaxed) && s.last_trace.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
        s.last_trace = Some(Instant::now());
        let d = |a: usize| unsafe { (a as *const i32).read_volatile() };
        log!(
            "monsterous: scene {scene} level {} credits {} shots {}/{}/{} release {} state {:#x} mode {} hits {:?}",
            d(0x6074a0), read_i16(CREDITS), read_i16(0x6065f8), read_i16(0x6065e0), read_i16(0x6065f2), d(0x6074b0),
            d(0x6074c8), d(0x6077d8), (0..TARGETS).map(|i| d(0x42d8b0 + 4 * i)).collect::<Vec<_>>()
        );
    }
    let fire = s.held.contains(&Native::Fire) && scene <= 1;
    for n in pressed {
        match n {
            Native::Menu => {
                s.menu_key = !s.menu_key;
                switches ^= MENU_KEY;
                log!("monsterous: operator menu {}", if s.menu_key { "on" } else { "off" });
            }
            Native::Coin | Native::Service => coin_in(),
            Native::Up | Native::Down | Native::Fire if menu => {}
            Native::Up | Native::Down | Native::Left | Native::Right => {
                s.aim = step(&targets(), s.aim, n);
                AIM.store(s.aim, Ordering::Relaxed);
            }
            Native::Fire if scene >= 2 => unsafe { (SCENE_TIMER as *mut u32).write_volatile(1) },
            Native::Fire => {}
        }
    }
    unsafe { (OPERATOR as *mut u32).write_volatile(switches) };
    if fire && !menu {
        let t = Instant::now();
        if s.next_fire.is_none_or(|next| t >= next) {
            press(&mut s.queue, digit(s.aim));
            s.next_fire = Some(t + FIRE_REPEAT);
        }
    } else {
        s.next_fire = None;
    }
    if FREE_PLAY.load(Ordering::Relaxed) && !menu && read_i16(CREDITS) <= 0 {
        coin_in();
    }
}

static POLL_EVENT: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicU32 = AtomicU32::new(0);

/// SDLCALL is cdecl on win32.
unsafe extern "C" fn poll_event(event: *mut u8) -> i32 {
    let orig: unsafe extern "C" fn(*mut u8) -> i32 = unsafe { std::mem::transmute(POLL_EVENT.load(Ordering::Relaxed)) };
    let mut s = STATE.lock().unwrap();
    if s.queue.is_empty() {
        update(&mut s);
    }
    if event.is_null() {
        return i32::from(!s.queue.is_empty()) | unsafe { orig(event) };
    }
    if let Some((sym, pressed)) = s.queue.pop_front() {
        // SDL_KeyboardEvent: type, which, state, keysym @4 { scancode, sym @8, mod @12 }
        let mut e = [0u8; EVENT_SIZE];
        e[0] = if pressed { SDL_KEYDOWN } else { SDL_KEYUP };
        e[2] = if pressed { SDL_PRESSED } else { 0 };
        e[8..12].copy_from_slice(&sym.to_le_bytes());
        unsafe { std::ptr::copy_nonoverlapping(e.as_ptr(), event, EVENT_SIZE) };
        return 1;
    }
    drop(s);
    loop {
        if unsafe { orig(event) } == 0 {
            return 0;
        }
        // the real keyboard, mouse and joysticks
        let kind = unsafe { *event };
        if !(SDL_KEYDOWN..=SDL_JOYBUTTONUP).contains(&kind) {
            return 1;
        }
        if DROPPED.fetch_add(1, Ordering::Relaxed) == 0 {
            log!("monsterous: real input events dropped (first: type {kind}{})", if kind >= SDL_MOUSEMOTION { "" } else { ", a key" });
        }
    }
}

/// No key down for the game's own keyboard reads (its volume keys).
unsafe extern "C" fn get_key_state(numkeys: *mut i32) -> *const u8 {
    static KEYS: [u8; 512] = [0; 512];
    if !numkeys.is_null() {
        unsafe { *numkeys = 323 };
    }
    KEYS.as_ptr()
}

static FLIP: AtomicUsize = AtomicUsize::new(0);
static GET_KEY_STATE: AtomicUsize = AtomicUsize::new(0);
static MAP_RGB: AtomicUsize = AtomicUsize::new(0);

/// The aimed target's ring, drawn into the screen surface before it is shown.
unsafe extern "C" fn flip(screen: P) -> i32 {
    let f: unsafe extern "C" fn(P) -> i32 = unsafe { std::mem::transmute(FLIP.load(Ordering::Relaxed)) };
    let scene = read_i16(SCENE);
    if !screen.is_null() && read_i16(MENU_ON) == 0 && scene <= 1 {
        unsafe { draw_ring(screen as *const u8, targets()[AIM.load(Ordering::Relaxed)]) };
    }
    unsafe { f(screen) }
}

/// SDL_Surface (SDL 1.2, win32): flags, format @4, w @8, h @12, pitch (u16) @16, pixels @20;
/// SDL_PixelFormat: palette, BitsPerPixel @4, BytesPerPixel @5.
unsafe fn draw_ring(surface: *const u8, (cx, cy): (i32, i32)) {
    let map_rgb = MAP_RGB.load(Ordering::Relaxed);
    unsafe {
        let format = *(surface.add(4) as *const *const u8);
        let (w, h) = (*(surface.add(8) as *const i32), *(surface.add(12) as *const i32));
        let pitch = *(surface.add(16) as *const u16) as usize;
        let pixels = *(surface.add(20) as *const *mut u8);
        if format.is_null() || pixels.is_null() || *format.add(5) != 4 || map_rgb == 0 {
            return;
        }
        let map_rgb: unsafe extern "C" fn(*const u8, u8, u8, u8) -> u32 = std::mem::transmute(map_rgb);
        let (light, dark) = (map_rgb(format, 255, 236, 0), map_rgb(format, 40, 20, 0));
        for dy in -64..=64i32 {
            for dx in -64..=64i32 {
                let r2 = dx * dx + dy * dy;
                let c = if (52 * 52..58 * 58).contains(&r2) {
                    light
                } else if (49 * 49..52 * 52).contains(&r2) || (58 * 58..61 * 61).contains(&r2) {
                    dark
                } else {
                    continue;
                };
                let (x, y) = (cx + dx, cy + dy);
                if x >= 0 && y >= 0 && x < w && y < h {
                    *(pixels.add(y as usize * pitch + x as usize * 4) as *mut u32) = c;
                }
            }
        }
    }
}

static BIT_BLT: AtomicUsize = AtomicUsize::new(0);
static STRETCHED: AtomicBool = AtomicBool::new(false);

/// SDL's `windib` updates: its 1280x768 surface stretched to the window's client area.
unsafe extern "system" fn bit_blt(dc: P, x: i32, y: i32, cx: i32, cy: i32, src: P, x1: i32, y1: i32, rop: u32) -> i32 {
    let f: unsafe extern "system" fn(P, i32, i32, i32, i32, P, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(BIT_BLT.load(Ordering::Relaxed)) };
    let (mw, mh) = unsafe { (*(MODE_W as *const i32), *(MODE_H as *const i32)) };
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let hwnd = unsafe { WindowFromDC(dc as _) };
    if mw <= 0 || mh <= 0 || hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut r) } == 0 {
        return unsafe { f(dc, x, y, cx, cy, src, x1, y1, rop) };
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if (w, h) == (mw, mh) || w <= 0 || h <= 0 {
        return unsafe { f(dc, x, y, cx, cy, src, x1, y1, rop) };
    }
    if !STRETCHED.swap(true, Ordering::Relaxed) {
        log!("monsterous: picture {mw}x{mh} stretched to {w}x{h}");
    }
    let sx = |v: i32| (i64::from(v) * i64::from(w) / i64::from(mw)) as i32;
    let sy = |v: i32| (i64::from(v) * i64::from(h) / i64::from(mh)) as i32;
    unsafe {
        SetStretchBltMode(dc as _, COLORONCOLOR);
        StretchBlt(dc as _, sx(x), sy(y), sx(x + cx) - sx(x), sy(y + cy) - sy(y), src as _, x1, y1, cx, cy, rop)
    }
}

pub fn init() {
    TRACE.store(std::env::var("WAL_MONSTEROUS_TRACE").is_ok_and(|v| v == "1"), Ordering::Relaxed);
    if let Some(on) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
        FREE_PLAY.store(on, Ordering::Relaxed);
        log!("monsterous: free play {}", if on { "on" } else { "off" });
    }
    let hooks: [(&str, usize, &AtomicUsize); 3] = [
        ("SDL_PollEvent", poll_event as *const () as usize, &POLL_EVENT),
        ("SDL_Flip", flip as *const () as usize, &FLIP),
        ("SDL_GetKeyState", get_key_state as *const () as usize, &GET_KEY_STATE),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook("SDL.dll", name, hook) } {
            Some(o) => orig.store(o, Ordering::Relaxed),
            None => log!("monsterous: the executable does not import SDL.dll!{name}"),
        }
    }
    let sdl = unsafe { GetModuleHandleW(windows_sys::w!("SDL.dll")) };
    if sdl.is_null() {
        log!("monsterous: SDL.dll not loaded");
        return;
    }
    if let Some(f) = unsafe { GetProcAddress(sdl, c"SDL_MapRGB".as_ptr() as _) } {
        MAP_RGB.store(f as usize, Ordering::Relaxed);
    }
    match unsafe { iat::hook_module(sdl as usize, "GDI32.dll", "BitBlt", bit_blt as *const () as usize) } {
        Some(o) => BIT_BLT.store(o, Ordering::Relaxed),
        None => log!("monsterous: SDL.dll does not import GDI32.dll!BitBlt"),
    }
    log!("monsterous: SDL hooked (keys, the aimed target, the picture stretched)");
}
