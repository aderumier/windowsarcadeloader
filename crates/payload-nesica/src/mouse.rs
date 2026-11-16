//! The gun's mouse for a Source engine `inputsystem.dll`, from the virtual sticks.
//!
//! Left 4 Dead: Survivors' gun is a USB mouse: the engine's `inputsystem.dll` subclasses the
//! game window, reads the cursor from `WM_MOUSEMOVE` and recenters it every frame with
//! `SetCursorPos` (the look is the motion from the center); its buttons are the `MK_` flags of
//! any mouse button message (`MOUSE1`-`MOUSE5`: attack, shove, orders, reload, zoom). The real
//! keyboard and mouse messages are dropped from the message loops (`inputsystem.dll`'s and
//! `engine.dll`'s); the payload posts the cursor's motion and the buttons:
//!
//! * the look: the right stick (a speed), plus the mouse as a trackball (its wrapping position
//!   on the pedal axes, the profile's `mouse_wrap`: its motion as the cursor's);
//! * the buttons: `WAL_NESICA_MOUSE_BUTTONS=<virtual button>,...` (`MOUSE1`, `MOUSE2`...;
//!   `b1+start`: either; `none` or empty: never pressed);
//! * the wheel (the item change): `WAL_NESICA_MOUSE_WHEEL=<virtual button>,<virtual button>`,
//!   a notch toward the player (next item), a notch away, at each press.
//!
//! Options: `WAL_NESICA_MOUSE=<module>,...` (the modules: the first one's `SetCursorPos` is
//! followed, every one's message loop filtered), `WAL_NESICA_MOUSE_SPEED` (the right stick's
//! full tilt, pixels a second, default 1500), `WAL_NESICA_MOUSE_GAIN` (pixels a trackball
//! count, default 1).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::input_filter::{own_mouse, post_mouse, post_wheel};
use wal_payload_common::{gun, iat, input, log};
use wal_protocol::{Axis, button};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_MOUSEMOVE};

/// The buttons' `MK_` flags: left, right, middle, X1, X2.
const MK: [usize; 5] = [0x01, 0x02, 0x10, 0x20, 0x40];
/// The stick's dead zone (of 32767).
const DEAD_ZONE: f64 = 4000.0;
const PERIOD: Duration = Duration::from_millis(8);
/// The shortest press posted.
const MIN_PRESS: Duration = Duration::from_millis(120);

/// Where the game last put the cursor (client point).
static CENTER_X: AtomicI32 = AtomicI32::new(i32::MIN);
static CENTER_Y: AtomicI32 = AtomicI32::new(0);
/// The motion since, in 1/256 pixel.
static MOTION_X: AtomicIsize = AtomicIsize::new(0);
static MOTION_Y: AtomicIsize = AtomicIsize::new(0);

static BUTTONS: OnceLock<Vec<u32>> = OnceLock::new();
static WHEEL: OnceLock<Vec<u32>> = OnceLock::new();

/// A list of virtual button masks (`b1+start,b2,...`).
fn masks(name: &str) -> Vec<u32> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|item| item.split('+').filter_map(|b| button::from_name(b.trim())).fold(0, |m, b| m | b))
        .collect()
}

fn buttons() -> &'static [u32] {
    BUTTONS.get_or_init(|| masks("WAL_NESICA_MOUSE_BUTTONS"))
}

fn wheel() -> &'static [u32] {
    WHEEL.get_or_init(|| masks("WAL_NESICA_MOUSE_WHEEL"))
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// `SetCursorPos(x, y)` (screen point): the new center, the motion counted from there; the real
/// cursor stays where it is.
unsafe extern "system" fn set_cursor_pos(x: i32, y: i32) -> i32 {
    let mut at = POINT { x, y };
    let hwnd = gun::find_window();
    if !hwnd.is_null() {
        unsafe { ScreenToClient(hwnd, &mut at) };
    }
    CENTER_X.store(at.x, Ordering::Relaxed);
    CENTER_Y.store(at.y, Ordering::Relaxed);
    // whole pixels already reported: the fraction kept
    MOTION_X.fetch_and(0xFF, Ordering::Relaxed);
    MOTION_Y.fetch_and(0xFF, Ordering::Relaxed);
    1
}

/// The motion of a pedal axis carrying a wrapping trackball counter (0..32767) since `last`.
fn trackball(now: i16, last: &mut Option<i16>) -> f64 {
    let Some(was) = last.replace(now) else { return 0.0 };
    // modulo 32768: the shortest way round
    let d = (now as i32 - was as i32).rem_euclid(32768);
    (if d >= 16384 { d - 32768 } else { d }) as f64
}

/// A stick axis, -1..1 past its dead zone, its response curved (fine aim near the center).
fn tilt(v: i16) -> f64 {
    let v = v as f64;
    if v.abs() < DEAD_ZONE {
        return 0.0;
    }
    let t = ((v.abs() - DEAD_ZONE) / (32767.0 - DEAD_ZONE)).min(1.0);
    t * t * v.signum()
}

fn input_thread() {
    let speed = env_f64("WAL_NESICA_MOUSE_SPEED", 1500.0) * PERIOD.as_secs_f64() * 256.0;
    let gain = env_f64("WAL_NESICA_MOUSE_GAIN", 1.0) * 256.0;
    let (mut mouse_x, mut mouse_y) = (None, None);
    let mut held = 0usize;
    let mut turning = [false; 2];
    let mut pressed_at: [Option<Instant>; 5] = [None; 5];
    loop {
        std::thread::sleep(PERIOD);
        let stick = input(0);
        let dx = tilt(stick.axis(Axis::RightX)) * speed + trackball(stick.axis(Axis::Accel), &mut mouse_x) * gain;
        let dy = tilt(stick.axis(Axis::RightY)) * speed + trackball(stick.axis(Axis::Brake), &mut mouse_y) * gain;
        let hwnd = gun::find_window();
        let Some((_, _, w, h)) = (!hwnd.is_null()).then(|| gun::client_area(hwnd)).flatten() else { continue };
        // the menus' cursor, before the game first recenters it: from the window's center
        if CENTER_X.load(Ordering::Relaxed) == i32::MIN {
            CENTER_Y.store(h as i32 / 2, Ordering::Relaxed);
            CENTER_X.store(w as i32 / 2, Ordering::Relaxed);
        }
        // a press held at least MIN_PRESS: the game reads the buttons' state once a frame, a
        // mouse click's press and release in the same frame were no click
        let mut flags = 0;
        for ((mask, mk), since) in buttons().iter().zip(MK).zip(pressed_at.iter_mut()) {
            if *mask != 0 && stick.pressed(*mask) {
                since.get_or_insert_with(Instant::now);
            } else if since.is_some_and(|t| t.elapsed() >= MIN_PRESS) {
                *since = None;
            }
            if since.is_some() {
                flags |= mk;
            }
        }
        // the cursor kept in the window (the menus' cursor is not recentered): the motion past
        // its edges dropped
        let (cx, cy) = (CENTER_X.load(Ordering::Relaxed) as isize, CENTER_Y.load(Ordering::Relaxed) as isize);
        let clamp = |motion: &AtomicIsize, d: f64, center: isize, size: f32| {
            let m = (motion.load(Ordering::Relaxed) + d as isize).clamp(-center << 8, ((size as isize - 1 - center) << 8) | 0xFF);
            motion.store(m, Ordering::Relaxed);
            (center + (m >> 8)) as i32
        };
        let (x, y) = (clamp(&MOTION_X, dx, cx, w), clamp(&MOTION_Y, dy, cy, h));
        if dx as isize != 0 || dy as isize != 0 {
            post_mouse(hwnd, WM_MOUSEMOVE, x, y, flags);
            // the desktop's pointer (the menus' cursor) put on the payload's cursor: what is
            // seen is where it clicks
            let real = ORIG_SET_CURSOR_POS.load(Ordering::Relaxed);
            if real != 0 {
                let mut at = POINT { x, y };
                unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at) };
                let f: unsafe extern "system" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(real) };
                unsafe { f(at.x, at.y) };
            }
        }
        // any button message sets every button from its flags
        if flags != held {
            if std::env::var("WAL_NESICA_MOUSE_TRACE").is_ok_and(|v| v == "1") {
                log!("mouse: buttons {flags:#x} at {x},{y}");
            }
            post_mouse(hwnd, WM_LBUTTONDOWN, x, y, flags);
            held = flags;
        }
        // a notch at each press: toward the player (negative), away
        for ((mask, delta), was) in wheel().iter().zip([-120i16, 120]).zip(turning.iter_mut()) {
            let now = *mask != 0 && stick.pressed(*mask);
            if now && !*was {
                post_wheel(hwnd, delta, x, y);
            }
            *was = now;
        }
    }
}

static ORIG_SET_CURSOR_POS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ORIG_WINDOW_FROM_POINT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `WindowFromPoint(point)`: the game window, wherever the payload's cursor is. The menus
/// (`vgui2.dll`) check the window under the cursor at a click; the desktop's windows there
/// (the real pointer elsewhere, another window above) made the click no click.
unsafe extern "system" fn window_from_point(point: POINT) -> windows_sys::Win32::Foundation::HWND {
    let hwnd = gun::find_window();
    if !hwnd.is_null() {
        return hwnd;
    }
    let f: unsafe extern "system" fn(POINT) -> windows_sys::Win32::Foundation::HWND = unsafe { std::mem::transmute(ORIG_WINDOW_FROM_POINT.load(Ordering::Relaxed)) };
    unsafe { f(point) }
}

static CALL_WINDOW_PROC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `WAL_NESICA_MOUSE_TRACE=1`: the mouse button messages the module's window procedure handled
/// (it passes each one on with `CallWindowProcA`).
unsafe extern "system" fn call_window_proc(proc_: usize, hwnd: usize, msg: u32, wparam: usize, lparam: isize) -> isize {
    if (0x201..=0x20D).contains(&msg) {
        log!("mouse: window message {msg:#x} wparam {wparam:#x} at {},{}", lparam as i16, (lparam >> 16) as i16);
    }
    let f: unsafe extern "system" fn(usize, usize, u32, usize, isize) -> isize = unsafe { std::mem::transmute(CALL_WINDOW_PROC.load(Ordering::Relaxed)) };
    unsafe { f(proc_, hwnd, msg, wparam, lparam) }
}

pub(crate) fn init() {
    let Ok(names) = std::env::var("WAL_NESICA_MOUSE") else { return };
    let modules: Vec<usize> = names
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .filter_map(|name| {
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            let base = unsafe { GetModuleHandleW(wide.as_ptr()) } as usize;
            if base == 0 {
                log!("mouse: {name} not loaded");
            }
            (base != 0).then_some(base)
        })
        .collect();
    let Some(&first) = modules.first() else { return };
    // the real keyboard and mouse dropped (the cabinet's controls come from the I/O board)
    wal_payload_common::input_filter::init(first, |_| false);
    for &m in &modules[1..] {
        wal_payload_common::input_filter::messages(m);
    }
    own_mouse(0);
    // every module of the game's, the ones it loads later too (the menus' vgui2.dll)
    wal_payload_common::late_hooks::hook("user32.dll", "SetCursorPos", set_cursor_pos as *const () as usize, &ORIG_SET_CURSOR_POS);
    wal_payload_common::late_hooks::hook("user32.dll", "WindowFromPoint", window_from_point as *const () as usize, &ORIG_WINDOW_FROM_POINT);
    if std::env::var("WAL_NESICA_MOUSE_TRACE").is_ok_and(|v| v == "1")
        && let Some(o) = unsafe { iat::hook_module(first, "user32.dll", "CallWindowProcA", call_window_proc as *const () as usize) }
    {
        CALL_WINDOW_PROC.store(o, Ordering::Relaxed);
    }
    buttons();
    wheel();
    std::thread::spawn(input_thread);
    log!("mouse: the gun's mouse of {names} from the virtual sticks");
}
