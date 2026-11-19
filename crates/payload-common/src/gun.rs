//! Where the players' guns point in the game's picture: the virtual sticks' `lx`/`ly` (the gun
//! on the screen, -1..1 from the top left) mapped onto the game's window (Unity's, Unreal's),
//! or onto the fake desktop's picture (`desktop.rs`) when the game runs on one.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_protocol::{Axis, StickState};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClientRect, GetSystemMetrics, GetWindowThreadProcessId, IsWindowVisible, SM_CXSCREEN, SM_CYSCREEN,
};

use crate::{desktop, input, log};

/// The game's window: a visible top-level Unity or Unreal window of this process, else its
/// largest visible top-level window (an older Unreal Engine 3's own class: Infinity Blade FX)
/// (0: none yet).
static WINDOW: AtomicUsize = AtomicUsize::new(0);

pub fn find_window() -> HWND {
    let current = WINDOW.load(Ordering::Relaxed) as HWND;
    if !current.is_null() && unsafe { IsWindowVisible(current) } != 0 {
        return current;
    }
    let pid = unsafe { GetCurrentProcessId() };
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
                log!("gun: game window {hwnd:?}");
                WINDOW.store(hwnd as usize, Ordering::Relaxed);
                return hwnd;
            }
        }
    }
    let hwnd = largest_window(pid);
    if !hwnd.is_null() {
        log!("gun: game window {hwnd:?} (largest of the process)");
        WINDOW.store(hwnd as usize, Ordering::Relaxed);
    }
    hwnd
}

/// This process's largest visible top-level window (client area).
fn largest_window(pid: u32) -> HWND {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
    struct Search {
        pid: u32,
        best: HWND,
        area: f32,
    }
    unsafe extern "system" fn each(hwnd: HWND, search: isize) -> i32 {
        let search = unsafe { &mut *(search as *mut Search) };
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == search.pid
            && unsafe { IsWindowVisible(hwnd) } != 0
            && let Some((_, _, w, h)) = client_area(hwnd)
            && w * h > search.area
        {
            search.best = hwnd;
            search.area = w * h;
        }
        1
    }
    let mut search = Search { pid, best: std::ptr::null_mut(), area: 0.0 };
    unsafe { EnumWindows(Some(each), &mut search as *mut Search as isize) };
    search.best
}

/// The game window's client area on the screen: left, top, width, height.
pub fn client_area(hwnd: HWND) -> Option<(f32, f32, f32, f32)> {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut origin = POINT { x: 0, y: 0 };
    if hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut r) } == 0 || unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
        return None;
    }
    (r.right > 0 && r.bottom > 0).then(|| (origin.x as f32, origin.y as f32, r.right as f32, r.bottom as f32))
}

/// The gun position of `stick` (`lx`, `ly`) in screen pixels, for a `screen` size.
pub fn on_screen(stick: &StickState, screen: (f32, f32)) -> (f32, f32) {
    let x = (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0 * screen.0;
    let y = (stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0 * screen.1;
    (x, y)
}

/// Where player 1's gun points in the game's picture (fractions from its top left), on it.
pub fn gun_in_picture() -> Option<(f32, f32)> {
    gun_of_player_in_picture(0)
}

/// Where `player`'s gun points in the game's picture (fractions from its top left), on it.
pub fn gun_of_player_in_picture(player: usize) -> Option<(f32, f32)> {
    in_picture(on_screen(&input(player), screen_size()))
}

/// Where `player`'s mouse points in the game's picture (fractions from its top left), on it, when
/// the profile maps it to the pedals (`input.gun`: `x: accel`, `y: brake`, 0..32767 across the
/// screen: the sticks left to other controls, CHUNITHM NEW!!'s slider).
pub fn pedals_in_picture(player: usize) -> Option<(f32, f32)> {
    let stick = input(player);
    let full = |v: i16| (v.max(0) as i32 * 2 - 32768).clamp(-32768, 32767) as i16;
    let mut as_stick = StickState::default();
    as_stick.axes[Axis::LeftX as usize] = full(stick.axis(Axis::Accel));
    as_stick.axes[Axis::LeftY as usize] = full(stick.axis(Axis::Brake));
    in_picture(on_screen(&as_stick, screen_size()))
}

/// A screen place in the game's picture (fractions from its top left), on it.
fn in_picture((sx, sy): (f32, f32)) -> Option<(f32, f32)> {
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

/// A touch pointer that is either the mouse or the left stick, whichever moved last (Border
/// Break Scramble: its left stick also walks the mech, so the mouse is mapped to the pedals,
/// `accel` / `brake` 0..32767 across the screen, which the game does not read). Player 1's place
/// in the stick's -32768..32767.
pub fn pedals_or_stick() -> (i16, i16) {
    static LAST: PedalsOr = PedalsOr::new();
    LAST.place(Axis::LeftX, Axis::LeftY)
}

/// The same with the right stick (Shining Force Cross: its left stick moves the character, the
/// right one is free).
pub fn pedals_or_right_stick() -> (i16, i16) {
    static LAST: PedalsOr = PedalsOr::new();
    LAST.place(Axis::RightX, Axis::RightY)
}

/// The pedals (the mouse) and a stick at the last call, and which of them moved last.
struct PedalsOr {
    last: std::sync::Mutex<Option<[(i16, i16); 2]>>,
    pedals_last: std::sync::atomic::AtomicBool,
}

impl PedalsOr {
    const fn new() -> PedalsOr {
        PedalsOr { last: std::sync::Mutex::new(None), pedals_last: std::sync::atomic::AtomicBool::new(false) }
    }

    fn place(&self, x: Axis, y: Axis) -> (i16, i16) {
        let stick = input(0);
        let full = |v: i16| (v.max(0) as i32 * 2 - 32768).clamp(-32768, 32767) as i16;
        let pedals = (full(stick.axis(Axis::Accel)), full(stick.axis(Axis::Brake)));
        let other = (stick.axis(x), stick.axis(y));
        let mut last = self.last.lock().unwrap();
        if let Some([was_pedals, was_other]) = *last {
            if pedals != was_pedals {
                self.pedals_last.store(true, Ordering::Relaxed);
            } else if other != was_other {
                self.pedals_last.store(false, Ordering::Relaxed);
            }
        }
        *last = Some([pedals, other]);
        if self.pedals_last.load(Ordering::Relaxed) { pedals } else { other }
    }
}

/// The screen's size in pixels.
pub fn screen_size() -> (f32, f32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) }
}
