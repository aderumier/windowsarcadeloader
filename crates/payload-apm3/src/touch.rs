//! A touch screen game (`WAL_APM3_TOUCH=1`: Otoshu DX, Unity 2018 IL2CPP). The screen's touches
//! are player 1's gun (the mouse, or the pointer `lx` / `ly`) and the `touch` input (its
//! trigger, the profile's `native_map`), posted as the game window's left mouse button: Unity
//! reads touch screens' presses as its mouse. The real mouse and keyboard never reach the game
//! (`input_filter`, the Unity player's cursor polls answered with the payload's cursor).

use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse, post_raw_buttons};
use wal_payload_common::{gun, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;

pub(crate) fn enabled() -> bool {
    crate::env("WAL_APM3_TOUCH").is_some_and(|v| v == "1")
}

pub(crate) fn start() {
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    wal_payload_common::input_filter::init(unity, |_| false);
    own_mouse(unity);
    log!("touch: player 1's gun touches the screen (Unity player {unity:#x})");
    std::thread::spawn(|| {
        let (mut was, mut last) = (false, (i32::MIN, i32::MIN));
        loop {
            std::thread::sleep(Duration::from_millis(8));
            let hwnd = gun::find_window();
            if hwnd.is_null() {
                continue;
            }
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut r) };
            if let Some((x, y)) = gun::gun_in_picture() {
                let at = ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32);
                if at != last {
                    last = at;
                    post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, if was { MK_LBUTTON } else { 0 });
                }
            }
            let touch = crate::touch_held();
            if touch != was && last.0 != i32::MIN {
                log!("touch: {} at {:?}", if touch { "down" } else { "up" }, last);
                post_mouse(hwnd, if touch { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, last.0, last.1, if touch { MK_LBUTTON } else { 0 });
                post_raw_buttons(hwnd, if touch { 1 } else { 2 });
            }
            was = touch;
        }
    });
}

