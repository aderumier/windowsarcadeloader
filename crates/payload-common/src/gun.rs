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

/// The game's window: a visible top-level Unity or Unreal window of this process (0: none yet).
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
    std::ptr::null_mut()
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
pub fn screen_size() -> (f32, f32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) }
}
