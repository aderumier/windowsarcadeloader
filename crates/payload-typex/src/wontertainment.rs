//! Won!Tertainment Music Channel's cabinet devices (`WAL_TYPEX_WONTERTAINMENT=1`).
//!
//! * COM3: the card scanner, a barcode reader sending each card's code unprompted: 12 hex
//!   digits, CR, LF (9600 baud, 8N1). Without the port: error 600 (card scanner error). The
//!   virtual card and its picker: `wontertainment_cards.rs`.
//! * COM1 (9600 baud, 8E1): the infrared link to the Idol Puppy, the robot dog toy dancing on
//!   the cabinet. Frames `FD 02 ~(code + seq) code seq [data]`; the game's presence query `7E`
//!   wants the toy's `7E` frame back (its two state bytes equal, then 0xAA: ready), without it
//!   error 700 (Idol Puppy communication error); its other frames get the toy's `AA` ack.
//! * The touch screen: on the cabinet a touch panel driver (`Driver\tpdrv.exe`) moves the mouse;
//!   the game reads `WM_MOUSEMOVE` (the cursor's place over its client area) and the left button.
//!   The gun (`lx` / `ly`: the mouse or a lightgun) is posted as the mouse over the picture, the
//!   touch on `b3`; the real mouse and keyboard never reach the game (`input_filter`).
//!
//! The JVS switches and the card dispenser: `WAL_TYPEX_JVS_LAYOUT=wontertainment`.

use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse};
use wal_payload_common::{gun, input, input_filter, log, screenshot, serial};
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
    if !std::env::var("WAL_TYPEX_WONTERTAINMENT").is_ok_and(|v| v == "1") {
        return;
    }
    serial::install("COM3", |_| Vec::new());
    serial::install("COM1", toy);
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    input_filter::init(exe, |_| false);
    own_mouse(exe);
    std::thread::spawn(touch_thread);
    crate::wontertainment_cards::init();
    log!("wontertainment: card scanner on COM3, Idol Puppy on COM1, touch screen from the gun");
}

/// The Idol Puppy's replies.
fn toy(packet: &[u8]) -> Vec<u8> {
    let frame = |code: u8, seq: u8, data: &[u8]| {
        let mut f = vec![0xFD, 0x02, !code.wrapping_add(seq), code, seq];
        f.extend_from_slice(data);
        f
    };
    if packet.len() < 5 || packet[0] != 0xFD || packet[1] != 0x02 {
        return Vec::new();
    }
    let (code, seq) = (packet[3], packet[4]);
    match code {
        0x7E => frame(0x7E, seq, &[seq, 0xAA]),
        _ => frame(0xAA, seq, &[]),
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

/// The gun posted as the mouse over the game's picture, `b3` its left button (the touch).
fn touch_thread() {
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
        let touch = stick.pressed(button::B3);
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
