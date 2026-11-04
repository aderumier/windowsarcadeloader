//! A touch screen game played with the mouse (`WAL_ADRENALINE_MOUSE=1`: Candy Crush Arcade,
//! Unity 4.6, 32-bit, Direct3D 11).
//!
//! - **Where.** The real cursor's place (Wine's: the desktop's mouse, or a lightgun moving it):
//!   the launcher's mouse position adds up the mouse's raw motion, never where the desktop's
//!   cursor is. On the game's picture, in the fake desktop's pixels (`WAL_DESKTOP_SIZE`: Unity
//!   takes the cursor's client coordinates as its screen's pixels).
//! - **Presses.** The launcher's player 1 `touch` native (the real mouse never reaches the game:
//!   `payload-common`'s `input_filter`).
//! - **Touches.** The candies take TouchScript's touches only (`TouchScript.Windows`: both
//!   `Win7TouchInput`, `WM_TOUCH`, and `Win8TouchInput`, `WM_POINTER*`, enabled; either turns its
//!   `MouseInput` off): the presses, moves and releases are posted as one finger's `WM_TOUCH`
//!   (`WM_POINTER*` without the Win7 source), their `DllImport`s (`GetTouchInputInfo`,
//!   `GetPointerInfo`...) answered by the payload. `GetForegroundWindow` is the game's window
//!   (the one TouchScript subclasses; an unmanaged window is never the foreground).
//! - **Mouse.** The menus (NGUI's `UICamera`) read Unity's mouse: the moves and presses posted
//!   as mouse messages too, the buttons also as raw mouse reports (Unity 4 reads them from raw
//!   input: `input_filter::post_raw_buttons`).
//! - **Pointer.** A dot where it touches (the system's arrow hidden by the profile's
//!   `WAL_HIDE_CURSOR`: the dump's assembly shows it) (the common crosshair option, on by default:
//!   `WAL_CROSSHAIR_SIZE` its diameter in percent of the picture's height, 1.5;
//!   `WAL_CROSSHAIR_COLOR` `RRGGBB`, red).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse, post_raw_buttons};
use wal_payload_common::{desktop, gun, log, pointer_d3d11};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

use crate::{Native, held};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;

pub fn enabled() -> bool {
    std::env::var("WAL_ADRENALINE_MOUSE").is_ok_and(|v| v.trim() == "1")
}

fn input_thread() {
    let (mut touch_was, mut last) = (false, (i32::MIN, i32::MIN));
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        match desktop::size() {
            Some((w, h)) => (r.right, r.bottom) = (w, h),
            None => {
                unsafe { GetClientRect(hwnd, &mut r) };
            }
        }
        let at = match cursor_in_picture() {
            Some((x, y)) => ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32),
            None => last,
        };
        let touch = held(0, Native::Touch);
        let buttons = if touch { MK_LBUTTON } else { 0 };
        let moved = at != last && at.0 != i32::MIN;
        if moved {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if touch != touch_was {
            log!("mouse: touch {} at {:?}", if touch { "down" } else { "up" }, last);
        }
        if touch && touch_was && moved && last.0 != i32::MIN {
            post_touch(hwnd, TOUCH_MOVE, last);
        }
        if touch != touch_was && last.0 != i32::MIN {
            post_touch(hwnd, if touch { TOUCH_DOWN } else { TOUCH_UP }, last);
            post_mouse(hwnd, if touch { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, last.0, last.1, buttons);
            // Unity 4: the buttons from raw input (RI_MOUSE_LEFT_BUTTON_DOWN / _UP)
            post_raw_buttons(hwnd, if touch { 1 } else { 2 });
        }
        touch_was = touch;
    }
}

/// Where the real cursor is on the game's picture (fractions from its top left), on it: the
/// payload's own `GetCursorPos` import is the real one (the game's answers the payload's cursor).
fn cursor_in_picture() -> Option<(f32, f32)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    let (x, y) = match desktop::size() {
        Some((w, h)) => {
            let (x, y) = desktop::from_screen(p.x as f64, p.y as f64);
            (x as f32 / w as f32, y as f32 / h as f32)
        }
        None => {
            let hwnd = gun::find_window();
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if hwnd.is_null() || unsafe { GetWindowRect(hwnd, &mut r) } == 0 || r.right <= r.left || r.bottom <= r.top {
                return None;
            }
            ((p.x - r.left) as f32 / (r.right - r.left) as f32, (p.y - r.top) as f32 / (r.bottom - r.top) as f32)
        }
    };
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

// The touch screen: one finger, TouchScript's Windows touch sources

const TOUCH_MOVE: u32 = 1;
const TOUCH_DOWN: u32 = 2;
const TOUCH_UP: u32 = 4;
/// `TOUCHEVENTF_PRIMARY`.
const TOUCH_PRIMARY: u32 = 0x10;
const WM_TOUCH: u32 = 0x0240;
const WM_POINTERUPDATE: u32 = 0x0245;
const WM_POINTERDOWN: u32 = 0x0246;
const WM_POINTERUP: u32 = 0x0247;
/// The finger's pointer id (`WM_POINTER*`) and touch id (`WM_TOUCH`).
const FINGER: u32 = 1;
/// The `HTOUCHINPUT` handles posted: the mark and a slot of the event.
const TOUCH_MARK: usize = 0x57A2_0000;
const TOUCH_SLOTS: usize = 64;

/// The source the game registered: `RegisterTouchWindow` (Win7), `EnableMouseInPointer` (Win8).
static WIN7: AtomicBool = AtomicBool::new(false);
static WIN8: AtomicBool = AtomicBool::new(false);
static TOUCH_SEQ: AtomicUsize = AtomicUsize::new(0);
/// Each posted `WM_TOUCH`'s event: flags, screen x, y.
static TOUCH_EVENTS: [[AtomicI32; 3]; TOUCH_SLOTS] = [const { [const { AtomicI32::new(0) }; 3] }; TOUCH_SLOTS];
/// The finger's last event (`GetPointerInfo`): flags, screen x, y.
static FINGER_STATE: [AtomicI32; 3] = [const { AtomicI32::new(0) }; 3];

/// Posts the finger's event `flags` at client point `at` (the fake desktop's pixels; the screen
/// point TouchScript's `ScreenToClient` turns back into them).
fn post_touch(hwnd: windows_sys::Win32::Foundation::HWND, flags: u32, at: (i32, i32)) {
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
    let mut p = POINT { x: at.0, y: at.1 };
    unsafe { ClientToScreen(hwnd, &mut p) };
    for (slot, v) in FINGER_STATE.iter().zip([flags as i32, p.x, p.y]) {
        slot.store(v, Ordering::Relaxed);
    }
    if WIN7.load(Ordering::Relaxed) {
        let seq = TOUCH_SEQ.fetch_add(1, Ordering::Relaxed) % TOUCH_SLOTS;
        for (slot, v) in TOUCH_EVENTS[seq].iter().zip([(flags | TOUCH_PRIMARY) as i32, p.x, p.y]) {
            slot.store(v, Ordering::Relaxed);
        }
        unsafe { PostMessageW(hwnd, WM_TOUCH, 1, (TOUCH_MARK | seq) as isize) };
    } else if WIN8.load(Ordering::Relaxed) {
        // both sources subclass the window (Win7's first): one touch only
        let message = match flags {
            TOUCH_DOWN => WM_POINTERDOWN,
            TOUCH_UP => WM_POINTERUP,
            _ => WM_POINTERUPDATE,
        };
        let lparam = ((p.y as u16 as isize) << 16) | p.x as u16 as isize;
        unsafe { PostMessageW(hwnd, message, FINGER as usize, lparam) };
    }
}

/// The game's `user32.dll` `DllImport`s of TouchScript's Windows touch sources.
pub fn resolve(name: &str) -> Option<usize> {
    Some(match name {
        "GetForegroundWindow" => get_foreground_window as *const () as usize,
        "RegisterTouchWindow" => {
            WIN7.store(true, Ordering::Relaxed);
            log!("mouse: TouchScript's WM_TOUCH input: the touches posted");
            register_touch_window as *const () as usize
        }
        "GetTouchInputInfo" => get_touch_input_info as *const () as usize,
        "CloseTouchInputHandle" => close_touch_input_handle as *const () as usize,
        "EnableMouseInPointer" => {
            WIN8.store(true, Ordering::Relaxed);
            log!("mouse: TouchScript's WM_POINTER input: the touches posted");
            enable_mouse_in_pointer as *const () as usize
        }
        "GetPointerInfo" => get_pointer_info as *const () as usize,
        _ => return None,
    })
}

unsafe extern "system" fn get_foreground_window() -> *mut c_void {
    let hwnd = gun::find_window();
    if hwnd.is_null() { unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } } else { hwnd }
}

unsafe extern "system" fn register_touch_window(_hwnd: *mut c_void, _flags: u32) -> i32 {
    1
}

unsafe extern "system" fn close_touch_input_handle(_input: *mut c_void) -> i32 {
    1
}

unsafe extern "system" fn enable_mouse_in_pointer(_enable: i32) -> i32 {
    1
}

/// `TOUCHINPUT`: x, y (hundredths of screen pixels), hSource, dwID, dwFlags, dwMask, dwTime,
/// dwExtraInfo, cxContact, cyContact.
unsafe extern "system" fn get_touch_input_info(input: *mut c_void, count: u32, inputs: *mut u8, size: i32) -> i32 {
    let h = input as usize;
    let ptr = size_of::<usize>();
    if h & !0xFFFF != TOUCH_MARK || count == 0 || inputs.is_null() || (size as usize) < 32 + 2 * ptr {
        return 0;
    }
    let [flags, x, y] = TOUCH_EVENTS[h % TOUCH_SLOTS].each_ref().map(|v| v.load(Ordering::Relaxed));
    unsafe {
        std::ptr::write_bytes(inputs, 0, size as usize);
        let put = |at: usize, v: i32| (inputs.add(at) as *mut i32).write_unaligned(v);
        put(0, x * 100);
        put(4, y * 100);
        put(8 + ptr, FINGER as i32);
        put(12 + ptr, flags);
    }
    1
}

/// `POINTER_INFO`: pointerType (PT_TOUCH 2), pointerId, frameId, pointerFlags, sourceDevice,
/// hwndTarget, ptPixelLocation...
unsafe extern "system" fn get_pointer_info(id: u32, info: *mut u8) -> i32 {
    if id != FINGER || info.is_null() {
        return 0;
    }
    let ptr = size_of::<usize>();
    let [flags, x, y] = FINGER_STATE.each_ref().map(|v| v.load(Ordering::Relaxed));
    // POINTER_FLAG_INCONTACT 4, _DOWN 0x10000, _UP 0x40000, _UPDATE 0x20000
    let pointer_flags: u32 = match flags as u32 {
        TOUCH_DOWN => 0x1_0004,
        TOUCH_UP => 0x4_0000,
        _ => 0x2_0004,
    };
    unsafe {
        let put = |at: usize, v: i32| (info.add(at) as *mut i32).write_unaligned(v);
        put(0, 2);
        put(4, FINGER as i32);
        put(12, pointer_flags as i32);
        put(16 + 2 * ptr, x);
        put(20 + 2 * ptr, y);
    }
    1
}

/// `RRGGBB` as red, green, blue (0..1).
fn color() -> Option<[f32; 3]> {
    let v = std::env::var("WAL_CROSSHAIR_COLOR").ok()?;
    let v = v.trim().trim_start_matches('#');
    let n = u32::from_str_radix(v, 16).ok().filter(|_| v.len() == 6)?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8].map(|c| c as f32 / 255.0))
}

/// `unity`: the module whose `GetCursorPos` is the payload's (the Unity 4 executable).
pub fn init(unity: usize) {
    own_mouse(unity);
    std::thread::spawn(input_thread);
    if wal_payload_common::crosshairs(true) {
        let size = std::env::var("WAL_CROSSHAIR_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.5);
        pointer_d3d11::init(pointer_d3d11::Dot {
            color: color().unwrap_or([1.0, 0.125, 0.125]),
            size: (size / 100.0).clamp(0.005, 0.2),
            position: cursor_in_picture,
            crosshair: false,
        });
    }
    log!("mouse: player 1's gun is the game's mouse");
}
