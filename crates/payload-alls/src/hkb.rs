//! Chrono Regalia's satellite seat (`WAL_ALLS_BOARD=hkb`, game id SDEC).
//!
//! * The touch screen: the game reads Windows touches when its touch panel is enabled (Amptw's
//!   `AmptWinstd`, error 5501 without a digitizer), else the mouse (Unity's
//!   `Input.mousePosition` and left button: `TouchPanelManager.ScreenInputType.Mouse`). The
//!   profile's configuration (`HkbSystemConfig_wal.json`, the game's `-c`) disables the touch
//!   panel and the LED board: the gun (`lx` / `ly`: the mouse or a lightgun) is posted as the
//!   mouse over the picture, the touch on `b1`; the real mouse and keyboard never reach the game
//!   (`input_filter`).
//! * The game server (`server.rs`): the Aime card players' data, kept in `Y:\hkb\`.
//! * The clock: the game's Mono (`mono.dll`, loaded by the Unity player) reads the time and time
//!   zone itself; the cabinet's clock hooks (`clock.rs`: Japan's time, `WAL_ALLS_TIMEWARP`) go
//!   in its imports too, else the store's closing hours (the operator's setting, 24:00 to the
//!   game day's 07:00 start by default) followed the PC's own time zone.

mod master;
mod server;
mod user;

use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse};
use wal_payload_common::{gun, input, log, screenshot};
use wal_protocol::button;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetWindowThreadProcessId, IsWindowVisible, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;

pub(crate) fn init() {
    server::init();
    std::thread::spawn(mono_clock);
    std::thread::spawn(touch_thread);
    log!("alls: hkb: touch screen from the gun (b1 touches)");
}

/// The clock hooks in Mono's imports, as soon as the Unity player loads it (before the game's
/// scripts read the time zone: Mono keeps the first one).
fn mono_clock() {
    if std::env::var("WAL_ALLS_CLOCK").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    for _ in 0..30_000 {
        let mono = unsafe { GetModuleHandleW(windows_sys::core::w!("mono.dll")) } as usize;
        if mono != 0 {
            crate::clock::hook_module(mono);
            log!("alls: hkb: clock hooks in mono.dll");
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The game's window: a visible top-level window of this process.
fn game_window() -> HWND {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) } != 0 {
            unsafe { *(found as *mut HWND) = hwnd };
            return 0;
        }
        1
    }
    let mut found: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
    found
}

/// The gun posted as the mouse over the game's picture, `b1` its left button (the touch).
fn touch_thread() {
    // the Unity player (hkb.exe's import): its cursor polls answer the payload's cursor
    loop {
        let unity = unsafe { GetModuleHandleW(windows_sys::core::w!("UnityPlayer.dll")) } as usize;
        if unity != 0 {
            own_mouse(unity);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (mut last, mut touched) = ((i32::MIN, i32::MIN), false);
    let mut hwnd: HWND = std::ptr::null_mut();
    loop {
        std::thread::sleep(Duration::from_millis(8));
        if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 {
            hwnd = game_window();
            if hwnd.is_null() {
                continue;
            }
        }
        let stick = input(0);
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(hwnd, &mut r) };
        let (sw, sh) = gun::screen_size();
        let (sx, sy) = gun::on_screen(&stick, (sw, sh));
        // the picture's place on the screen (letterboxed): the game takes its client area for it
        let (u, v) = screenshot::to_picture(sx / sw, sy / sh);
        let at = ((u * r.right as f32) as i32, (v * r.bottom as f32) as i32);
        let touch = stick.pressed(button::B1);
        let buttons = if touch { MK_LBUTTON } else { 0 };
        if at != last {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if touch != touched {
            post_mouse(hwnd, if touch { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, at.0, at.1, buttons);
            touched = touch;
        }
    }
}
