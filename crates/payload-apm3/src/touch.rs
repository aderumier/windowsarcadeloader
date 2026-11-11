//! A touch screen game (`WAL_APM3_TOUCH=1`: Otoshu DX, Unity 2018 IL2CPP). The screen's touches
//! are player 1's gun (the mouse, or the pointer `lx` / `ly`) and the `touch` input (its
//! trigger, the profile's `native_map`), posted as the game window's left mouse button: Unity
//! reads touch screens' presses as its mouse. The real mouse and keyboard never reach the game
//! (`input_filter`, the Unity player's cursor polls answered with the payload's cursor).
//! A dot where it touches over the Direct3D 11 picture (the game shows no cursor): the common
//! crosshair option (`WAL_CROSSHAIR=1`, off by default; `WAL_CROSSHAIR_SIZE` its diameter in
//! percent of the picture's height, 1.5; `WAL_CROSSHAIR_COLOR` `RRGGBB`, red).
//! The `pinch_in` / `pinch_out` inputs turn the mouse wheel where it touches, a notch every
//! 60 ms while held: the zoom of the games whose window takes the wheel as a two finger pinch
//! (Tapping Skill Test).

use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse, post_raw_buttons, post_wheel};
use wal_payload_common::{gun, log, pointer_d3d11};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;
/// A mouse wheel notch.
const WHEEL_DELTA: i16 = 120;
/// The touch thread's turns between two wheel notches of a held pinch (8 ms each).
const PINCH_TURNS: u32 = 8;

pub(crate) fn enabled() -> bool {
    crate::env("WAL_APM3_TOUCH").is_some_and(|v| v == "1")
}

pub(crate) fn start() {
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    wal_payload_common::input_filter::init(unity, |_| false);
    own_mouse(unity);
    log!("touch: player 1's gun touches the screen (Unity player {unity:#x})");
    if wal_payload_common::crosshairs(false) {
        let size = crate::env("WAL_CROSSHAIR_SIZE").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.5);
        let color = crate::env("WAL_CROSSHAIR_COLOR")
            .and_then(|v| u32::from_str_radix(&v, 16).ok().filter(|_| v.len() == 6))
            .map_or([1.0, 0.125, 0.125], |n| [(n >> 16) as u8, (n >> 8) as u8, n as u8].map(|c| c as f32 / 255.0));
        pointer_d3d11::init(pointer_d3d11::Dot {
            color,
            size: (size / 100.0).clamp(0.005, 0.2),
            position: gun::gun_in_picture,
            crosshair: false,
        });
    }
    std::thread::spawn(|| {
        let (mut was, mut last, mut pinch_turn) = (false, (i32::MIN, i32::MIN), 0u32);
        loop {
            std::thread::sleep(Duration::from_millis(8));
            let hwnd = gun::find_window();
            if hwnd.is_null() {
                continue;
            }
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut r) };
            // the game's client units: a stretched desktop's (`WAL_DESKTOP_CLIENT`)
            let (cw, ch) = wal_payload_common::desktop::client_size().unwrap_or((r.right, r.bottom));
            if let Some((x, y)) = gun::gun_in_picture() {
                let at = ((x * cw as f32) as i32, (y * ch as f32) as i32);
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
            match crate::pinch_held() {
                0 => pinch_turn = 0,
                dir if last.0 != i32::MIN => {
                    if pinch_turn % PINCH_TURNS == 0 {
                        post_wheel(hwnd, dir as i16 * WHEEL_DELTA, last.0, last.1);
                    }
                    pinch_turn += 1;
                }
                _ => {}
            }
        }
    });
}

