//! A touch panel read as the game window's left mouse button (JoJo's Bizarre Adventure: Unreal's
//! `UI_Touch`; Sword Art Online Arcade: Unity's `Input.GetMouseButton(0)`). Where: the real
//! cursor (the desktop's mouse or a lightgun moving it) on the game's window, posted as mouse
//! messages (`input_filter`: the real mouse never reaches the game); when: the game's `touch`
//! native, held in the flag given to [`init`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse};
use wal_payload_common::{gun, log, pointer_d3d11};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;

static HELD: OnceLock<&'static AtomicBool> = OnceLock::new();

/// The real cursor in the game window's client area (pixels), on it.
fn cursor() -> Option<(i32, i32, i32, i32)> {
    let hwnd = gun::find_window();
    let (mut p, mut r) = (POINT { x: 0, y: 0 }, RECT { left: 0, top: 0, right: 0, bottom: 0 });
    if hwnd.is_null() || unsafe { GetCursorPos(&mut p) } == 0 || unsafe { ScreenToClient(hwnd, &mut p) } == 0 || unsafe { GetClientRect(hwnd, &mut r) } == 0 {
        return None;
    }
    (p.x >= 0 && p.y >= 0 && p.x < r.right && p.y < r.bottom).then_some((p.x, p.y, r.right, r.bottom))
}

/// The touch position on the picture (fractions), for the pointer dot.
fn in_picture() -> Option<(f32, f32)> {
    cursor().map(|(x, y, w, h)| (x as f32 / w as f32, y as f32 / h as f32))
}

fn thread() {
    let (mut touched, mut last) = (false, (i32::MIN, i32::MIN));
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let at = cursor().map_or(last, |(x, y, _, _)| (x, y));
        let touch = HELD.get().is_some_and(|h| h.load(Ordering::Relaxed));
        let buttons = if touch { MK_LBUTTON } else { 0 };
        if at != last && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if touch != touched && last.0 != i32::MIN {
            post_mouse(hwnd, if touch { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, last.0, last.1, buttons);
            touched = touch;
        }
    }
}

/// The touch panel of the game whose window messages `module` reads (the executable, or
/// `UnityPlayer.dll`); `held`: the touch native's state, set by the game's I/O poll.
pub(crate) fn init(module: usize, held: &'static AtomicBool) {
    let _ = HELD.set(held);
    own_mouse(module);
    // the common pointer option: a dot where it touches
    if wal_payload_common::crosshairs(false) {
        let size = std::env::var("WAL_CROSSHAIR_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.5);
        pointer_d3d11::init(pointer_d3d11::Dot { color: [1.0, 0.125, 0.125], size: (size / 100.0).clamp(0.005, 0.2), position: in_picture, crosshair: false });
    }
    std::thread::spawn(thread);
    log!("touch: touch panel from the cursor");
}
