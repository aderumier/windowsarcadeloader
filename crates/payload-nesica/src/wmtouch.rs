//! Windows touches (`WM_TOUCH`) for the touch screen games (Tetote X Connect, Puzzle & Dragons
//! Battle Tournament): the game is told a multi-touch digitizer is ready (`GetSystemMetrics`),
//! its `RegisterTouchWindow` succeeds, and every 8 ms the fingers' presses, moves and releases
//! are posted as one `WM_TOUCH`, read back with the payload's `GetTouchInputInfo`.
//!
//! The fingers (up to 10): each player's gun (`lx` / `ly`: the mouse or a lightgun, players 1
//! and 2) while its `b1` is held, and the fingers on the player's touchpad or touchscreen (the
//! profile's `input.touch`): the whole pad is the picture, or with the game's surface option
//! (`screen`) a touchscreen whose part the picture is.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{desktop, gun, log};
use wal_protocol::button;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, PostMessageW};

type P = *mut c_void;

/// `TOUCHEVENTF_*`
const TOUCH_MOVE: u32 = 1;
const TOUCH_DOWN: u32 = 2;
const TOUCH_UP: u32 = 4;
const TOUCH_PRIMARY: u32 = 0x10;
const WM_TOUCH: u32 = 0x0240;
/// `SM_DIGITIZER`, `SM_MAXIMUMTOUCHES`
const SM_DIGITIZER: i32 = 94;
const SM_MAXIMUMTOUCHES: i32 = 95;
/// NID_INTEGRATED_TOUCH | NID_MULTI_INPUT | NID_READY
const DIGITIZER: i32 = 0x01 | 0x40 | 0x80;
const MAX_FINGERS: usize = 10;
/// The `HTOUCHINPUT` handles posted: the mark and a slot of the message.
const TOUCH_MARK: usize = 0x57A3_0000;
const TOUCH_SLOTS: usize = 64;

/// A finger's event: id, flags, screen x, y.
type Event = (u32, u32, i32, i32);
/// Each posted `WM_TOUCH`'s events.
static TOUCH_EVENTS: Mutex<Vec<Vec<Event>>> = Mutex::new(Vec::new());
static TOUCH_SEQ: AtomicUsize = AtomicUsize::new(0);
static REGISTERED: AtomicBool = AtomicBool::new(false);
/// When a finger last touched the screen.
static LAST_DOWN: Mutex<Option<Instant>> = Mutex::new(None);
/// The original `GetSystemMetrics` (the caller's hook stores it).
pub(crate) static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);

struct Config {
    /// The log lines' prefix (the game's module).
    name: &'static str,
    /// The game's surface option: `screen` a touchscreen, else a touchpad.
    surface_env: &'static str,
}

static CONFIG: std::sync::OnceLock<Config> = std::sync::OnceLock::new();

/// Starts posting the touches (once the game registered its window), `name` the log prefix,
/// `surface_env` the variable of the touch surface option.
pub(crate) fn start(name: &'static str, surface_env: &'static str) {
    if CONFIG.set(Config { name, surface_env }).is_ok() {
        std::thread::spawn(touch_loop);
    }
}

/// When a finger last touched the screen.
pub(crate) fn last_down() -> Option<Instant> {
    *LAST_DOWN.lock().unwrap()
}

pub(crate) unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    match index {
        SM_DIGITIZER => DIGITIZER,
        SM_MAXIMUMTOUCHES => MAX_FINGERS as i32,
        _ => {
            let orig: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
            unsafe { orig(index) }
        }
    }
}

pub(crate) unsafe extern "system" fn register_touch_window(_hwnd: HWND, _flags: u32) -> i32 {
    if !REGISTERED.swap(true, Ordering::Relaxed) {
        log!("{}: the game's touch window registered: the touches posted", CONFIG.get().map_or("wmtouch", |c| c.name));
    }
    1
}

#[cfg_attr(target_pointer_width = "64", allow(dead_code))]
pub(crate) unsafe extern "system" fn unregister_touch_window(_hwnd: HWND) -> i32 {
    1
}

pub(crate) unsafe extern "system" fn close_touch_input_handle(_input: P) -> i32 {
    1
}

/// `TOUCHINPUT` (48 bytes in 64-bit processes, 40 in 32-bit ones): x, y (hundredths of screen
/// pixels), hSource, dwID, dwFlags, dwMask, dwTime, dwExtraInfo, cxContact, cyContact.
pub(crate) unsafe extern "system" fn get_touch_input_info(input: P, count: u32, inputs: *mut u8, size: i32) -> i32 {
    const SIZE: usize = if cfg!(target_pointer_width = "64") { 48 } else { 40 };
    const ID: usize = 8 + size_of::<usize>();
    let h = input as usize;
    if h & !0xFFFF != TOUCH_MARK || inputs.is_null() || (size as usize) < SIZE {
        return 0;
    }
    let events = TOUCH_EVENTS.lock().unwrap();
    let Some(events) = events.get(h % TOUCH_SLOTS) else { return 0 };
    for (i, &(id, flags, x, y)) in events.iter().take(count as usize).enumerate() {
        unsafe {
            let at = inputs.add(i * size as usize);
            std::ptr::write_bytes(at, 0, size as usize);
            let put = |off: usize, v: i32| (at.add(off) as *mut i32).write_unaligned(v);
            put(0, x * 100);
            put(4, y * 100);
            put(ID, id as i32);
            put(ID + 4, flags as i32);
        }
    }
    1
}

/// The game window's client size in the units the game takes: the fake desktop's (the window
/// it made, `WAL_DESKTOP_SIZE`; its touches' client points, `ScreenToClient`, are taken in
/// them), else the window's.
fn client_size(hwnd: HWND) -> (i32, i32) {
    if let Some(size) = desktop::client_size().or_else(desktop::size) {
        return size;
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd, &mut r) };
    (r.right, r.bottom)
}

/// A point of the player's touch surface (0..65535 across it) in the game's picture: the whole
/// surface is the picture (a touchpad, by default), or with the surface option `screen` the
/// surface is the screen and the picture its part (a touchscreen: the finger where it shows).
fn surface_in_picture((x, y): (u16, u16)) -> Option<(f32, f32)> {
    let (x, y) = (x as f32 / 65535.0, y as f32 / 65535.0);
    static SCREEN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let screen = SCREEN.get_or_init(|| CONFIG.get().is_some_and(|c| std::env::var(c.surface_env).is_ok_and(|v| v.trim() == "screen")));
    if !*screen {
        return Some((x.min(0.9999), y.min(0.9999)));
    }
    let (sw, sh) = gun::screen_size();
    let (sx, sy) = (x * sw, y * sh);
    let (x, y) = match desktop::size() {
        Some((w, h)) => {
            let (x, y) = desktop::from_screen(sx as f64, sy as f64);
            (x as f32 / w as f32, y as f32 / h as f32)
        }
        None => {
            let (left, top, w, h) = gun::client_area(gun::find_window())?;
            ((sx - left) / w, (sy - top) / h)
        }
    };
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

/// The touch surface's fingers with ids kept while they stay down (the launcher sends their
/// points only): each point takes the id of the nearest finger of the last report, else a new
/// one (from 10).
fn surface_fingers(points: Vec<(f32, f32)>) -> Vec<(u32, (f32, f32))> {
    static LAST: Mutex<Vec<(u32, (f32, f32))>> = Mutex::new(Vec::new());
    let mut last = LAST.lock().unwrap();
    let mut free: Vec<(u32, (f32, f32))> = last.clone();
    let mut now = Vec::new();
    for p in points {
        let nearest = free
            .iter()
            .enumerate()
            .map(|(i, (_, q))| (i, (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|&(_, d)| d < 0.04)
            .map(|(i, _)| i);
        let id = match nearest {
            Some(i) => free.remove(i).0,
            None => (10..).find(|id| !last.iter().chain(now.iter()).any(|(l, _)| l == id)).unwrap_or(10),
        };
        now.push((id, p));
    }
    *last = now.clone();
    now
}

/// The fingers down now (id, picture fractions): the guns while their `b1` is held (ids 1, 2),
/// the touch surface's (ids 10..).
fn fingers() -> Vec<(u32, (f32, f32))> {
    let mut down = Vec::new();
    for p in 0..2 {
        if wal_payload_common::input(p).pressed(button::B1)
            && let Some(at) = gun::gun_of_player_in_picture(p)
        {
            down.push((p as u32 + 1, at));
        }
    }
    let points: Vec<(f32, f32)> = wal_payload_common::touches().into_iter().filter_map(surface_in_picture).collect();
    let room = MAX_FINGERS - down.len();
    down.extend(surface_fingers(points).into_iter().take(room));
    down
}

/// Every 8 ms: the fingers' presses, moves and releases posted as one `WM_TOUCH`.
fn touch_loop() {
    let name = CONFIG.get().map_or("wmtouch", |c| c.name);
    // the fingers down at the last message: id, screen point
    let mut last: Vec<(u32, (i32, i32))> = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() || !REGISTERED.load(Ordering::Relaxed) {
            continue;
        }
        let (cw, ch) = client_size(hwnd);
        let now: Vec<(u32, (i32, i32))> = fingers()
            .into_iter()
            .map(|(id, (x, y))| {
                let mut p = POINT { x: (x * cw as f32) as i32, y: (y * ch as f32) as i32 };
                unsafe { ClientToScreen(hwnd, &mut p) };
                (id, (p.x, p.y))
            })
            .collect();
        let mut events: Vec<Event> = Vec::new();
        for &(id, at) in &now {
            match last.iter().find(|(l, _)| *l == id) {
                None => events.push((id, TOUCH_DOWN, at.0, at.1)),
                Some((_, was)) if *was != at => events.push((id, TOUCH_MOVE, at.0, at.1)),
                Some(_) => {}
            }
        }
        for &(id, at) in &last {
            if !now.iter().any(|(n, _)| *n == id) {
                events.push((id, TOUCH_UP, at.0, at.1));
            }
        }
        if events.iter().any(|e| e.1 == TOUCH_DOWN) {
            *LAST_DOWN.lock().unwrap() = Some(Instant::now());
        }
        for e in events.iter().filter(|e| e.1 != TOUCH_MOVE) {
            log!("{name}: finger {} {} at {:?}", e.0, if e.1 == TOUCH_DOWN { "down" } else { "up" }, (e.2, e.3));
        }
        last = now;
        if events.is_empty() {
            continue;
        }
        events[0].1 |= TOUCH_PRIMARY;
        let count = events.len();
        let seq = TOUCH_SEQ.fetch_add(1, Ordering::Relaxed) % TOUCH_SLOTS;
        {
            let mut slots = TOUCH_EVENTS.lock().unwrap();
            if slots.len() < TOUCH_SLOTS {
                slots.resize(TOUCH_SLOTS, Vec::new());
            }
            slots[seq] = events;
        }
        unsafe { PostMessageW(hwnd, WM_TOUCH, count, (TOUCH_MARK | seq) as isize) };
    }
}
