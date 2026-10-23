//! Chase Chase Jokers' touch screen: the cabinet's main monitor is a touch screen, which the
//! game's menus read as Unity's mouse (`SubTouchInputModule`: `Input.mousePosition` and the left
//! button). UnityPlayer reads them from its window messages and `GetCursorPos`; its imports are
//! hooked:
//!
//! * `GetCursorPos`: the pointer, player 1's `accel` (x) and `brake` (y) axes over the window
//!   (a gun's or a mouse's position, `0..32767` across the screen).
//! * `PeekMessageA` / `GetMessageA`: the real keyboard and mouse messages become `WM_NULL`; the
//!   payload's own (`WM_MOUSEMOVE`, `WM_LBUTTONDOWN`, `WM_LBUTTONUP` for the native `touch`,
//!   tagged in `wParam`) are delivered.
//! * `GetAsyncKeyState`, `GetKeyState`: only the left button, while touching.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, MSG, PostMessageW, WM_KEYFIRST, WM_KEYLAST, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEFIRST, WM_MOUSELAST, WM_MOUSEMOVE, WM_NULL,
};

type P = *mut c_void;

/// `wParam` bit of the payload's mouse messages (no MK_ flag uses it).
const TAG: usize = 0x8000;
const MK_LBUTTON: usize = 1;
const VK_LBUTTON: i32 = 1;

static GET_CURSOR_POS: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static GET_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static WINDOW: AtomicUsize = AtomicUsize::new(0);

type GetCursorPos = unsafe extern "system" fn(*mut POINT) -> i32;
type PeekMessage = unsafe extern "system" fn(*mut MSG, HWND, u32, u32, u32) -> i32;
type GetMessage = unsafe extern "system" fn(*mut MSG, HWND, u32, u32) -> i32;

pub fn init() {
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    if unity == 0 {
        log!("ccj: UnityPlayer.dll not loaded: no touch screen");
        return;
    }
    let hooks: [(&str, usize, Option<&AtomicUsize>); 5] = [
        ("GetCursorPos", get_cursor_pos as *const () as usize, Some(&GET_CURSOR_POS)),
        ("PeekMessageA", peek_message_a as *const () as usize, Some(&PEEK_MESSAGE_A)),
        ("GetMessageA", get_message_a as *const () as usize, Some(&GET_MESSAGE_A)),
        ("GetAsyncKeyState", key_state as *const () as usize, None),
        ("GetKeyState", key_state as *const () as usize, None),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook_module(unity, "user32.dll", name, hook) } {
            Some(o) => {
                if let Some(slot) = orig {
                    slot.store(o, Ordering::Relaxed);
                }
            }
            None => log!("ccj: UnityPlayer does not import {name}"),
        }
    }
    std::thread::spawn(touch_loop);
}

/// The game's window, once it exists.
fn window() -> HWND {
    let mut w = WINDOW.load(Ordering::Relaxed) as HWND;
    if w.is_null() {
        w = crate::ccj_trackball::game_window();
        WINDOW.store(w as usize, Ordering::Relaxed);
    }
    w
}

/// The pointer in the window's client area.
fn pointer(window: HWND) -> (i32, i32) {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(window, &mut rect) };
    let stick = wal_payload_common::input(0);
    let at = |axis: Axis, size: i32| ((stick.axis(axis).max(0) as i64 * (size - 1).max(0) as i64) / 32767) as i32;
    (at(Axis::Accel, rect.right), at(Axis::Brake, rect.bottom))
}

/// Every 10 ms: the pointer's moves and the touch's presses, posted to the game's window.
fn touch_loop() {
    let mut last = (-1, -1);
    let mut down = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let w = window();
        if w.is_null() {
            continue;
        }
        let pos = pointer(w);
        let touch = crate::ccj::touching();
        let lparam = ((pos.1 as u32 & 0xffff) << 16 | (pos.0 as u32 & 0xffff)) as LPARAM;
        let buttons = if touch { MK_LBUTTON } else { 0 };
        if pos != last {
            last = pos;
            unsafe { PostMessageW(w, WM_MOUSEMOVE, TAG | buttons, lparam) };
        }
        if touch != down {
            down = touch;
            log!("ccj: touch {} at {pos:?}", if touch { "down" } else { "up" });
            let msg = if touch { WM_LBUTTONDOWN } else { WM_LBUTTONUP };
            if unsafe { PostMessageW(w, msg, TAG | buttons, lparam) } == 0 {
                WINDOW.store(0, Ordering::Relaxed);
            }
        }
    }
}

unsafe extern "system" fn get_cursor_pos(point: *mut POINT) -> i32 {
    let w = window();
    if w.is_null() || point.is_null() {
        let orig: GetCursorPos = unsafe { std::mem::transmute(GET_CURSOR_POS.load(Ordering::Relaxed)) };
        return unsafe { orig(point) };
    }
    let (x, y) = pointer(w);
    let mut p = POINT { x, y };
    unsafe {
        ClientToScreen(w, &mut p);
        *point = p;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| log!("ccj: Unity reads the pointer ({x}, {y}) -> {:?}", (p.x, p.y)));
    1
}

/// The real keyboard and mouse messages dropped, the payload's untagged.
unsafe fn filter(msg: *mut MSG) {
    let m = unsafe { &mut *msg };
    if (WM_KEYFIRST..=WM_KEYLAST).contains(&m.message) {
        m.message = WM_NULL;
    } else if (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&m.message) {
        if m.wParam & TAG != 0 {
            m.wParam &= !TAG;
        } else {
            m.message = WM_NULL;
        }
    }
}

unsafe extern "system" fn peek_message_a(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> i32 {
    let orig: PeekMessage = unsafe { std::mem::transmute(PEEK_MESSAGE_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(msg, hwnd, min, max, remove) };
    if r != 0 && !msg.is_null() {
        unsafe { filter(msg) };
    }
    r
}

unsafe extern "system" fn get_message_a(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32 {
    let orig: GetMessage = unsafe { std::mem::transmute(GET_MESSAGE_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(msg, hwnd, min, max) };
    if r > 0 && !msg.is_null() {
        unsafe { filter(msg) };
    }
    r
}

/// Only the left button, while touching.
unsafe extern "system" fn key_state(key: i32) -> i16 {
    if key == VK_LBUTTON && crate::ccj::touching() { i16::MIN } else { 0 }
}
