//! The game's Windows mouse from player 1's pointer (`WAL_RINGEDGE_MOUSE=1`): Puyo Puyo!! Quest
//! Arcade with hod5.ini's `[PqControl] s_Input = mouse` places its touches at the cursor
//! (`GetCursorPos`, then `ScreenToClient`, 0x5237cf) and reads the buttons from a DirectInput
//! system mouse (`GetDeviceState`, DIMOUSESTATE): the fake DirectInput's (`WAL_DINPUT_DISABLE=1`),
//! b8 its left button. The real mouse is dropped (its moves, buttons and cursor polls:
//! `input_filter`); the cursor follows the gun (or mouse) on the game's picture, the window's
//! messages posted too.

use std::time::Duration;

use wal_payload_common::input_filter::{self, own_mouse, post_mouse};
use wal_payload_common::{desktop, dinput, gun, input, log};
use wal_protocol::button;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

const MK_LBUTTON: usize = 0x0001;

pub(crate) fn init() {
    if !std::env::var("WAL_RINGEDGE_MOUSE").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    input_filter::init(exe, |_| false);
    own_mouse(exe);
    // the fake DirectInput's system keyboard (no key) and mouse (b8 the left button)
    dinput::set_keyboard(|_| {});
    dinput::set_mouse_buttons(|| input(0).pressed(button::B8) as u8);
    std::thread::spawn(mouse_thread);
    log!("mouse: the game's cursor from player 1's pointer, b8 its left button");
}

fn mouse_thread() {
    let mut last = (i32::MIN, i32::MIN);
    let mut down_was = false;
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        // the pointer's place on the picture, in the game's pixels (its desktop's on a stretched
        // desktop, whatever the window's size; kept at its last place off it)
        let (w, h) = desktop::size().unwrap_or_else(|| {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut r) };
            (r.right, r.bottom)
        });
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * w as f32) as i32, (y * h as f32) as i32),
            None => last,
        };
        if at.0 == i32::MIN {
            continue;
        }
        let down = input(0).pressed(button::B8);
        let buttons = if down { MK_LBUTTON } else { 0 };
        // again every half second: a cursor that did not move still reaches a new screen
        tick = (tick + 1) % 64;
        if at != last || tick == 0 {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if down != down_was {
            post_mouse(hwnd, if down { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, at.0, at.1, buttons);
            log!("mouse: {} at {},{}", if down { "press" } else { "release" }, at.0, at.1);
            down_was = down;
        }
    }
}
