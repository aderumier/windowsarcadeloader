//! Veggie Blast (UNIS, 2013, Virtools 3.x, 32-bit; `WAL_UNIS_BOARD=veggieblast`).
//!
//! The dump's `VeggieBlast.exe` is not the cabinet's player: a .NET 4 WinForms host that runs
//! the Virtools engine (`virtoolsview.dll`, `ck2.dll`) on `RecoveryBootstrap.nmo` and plays the
//! game's flow, aim and shots itself, its cabinet's UART board (COM1) left out. It reads:
//!
//! * the aim: the Windows cursor (`Cursor.Position`, polled each frame while its window has the
//!   focus) on its full screen window, else raw absolute mouse reports;
//! * the shot: `GetAsyncKeyState(VK_LBUTTON)` (held: a shot every 100 ms), the secondary
//!   (bomb) `VK_RBUTTON`;
//! * keys (`WM_KEYDOWN`): **1** / Enter start, **5** a credit once started (free play off),
//!   F1 / T its settings dialog, F4 the aiming mode, Escape quits;
//! * `LocalControls.ini` (next to it): `FreePlay`, `Easy` (slower targets, 90 s rounds),
//!   `Relative` (aim from mouse motion), `Sensitivity`.
//!
//! The payload plays them from player 1's virtual stick: the gun (or mouse) is the cursor
//! (user32's `NtUserGetCursorPos`: .NET's `DllImport`s reach no import hook), the trigger the
//! left button, b2 the right one, start / coin post 1 / 5; every message loop of the process
//! filtered (`input_filter::own_process_input`: the real keyboard and mouse never reach it).
//! `LocalControls.ini` gets the common free play option, `WAL_UNIS_VEGGIEBLAST_EASY` and
//! `Relative=False` (the gun points).
//!
//! Natives: `trigger`, `bomb` (player 1's mouse buttons), `start`, `coin`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_process_input, post_key, post_mouse, set_cursor};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, WM_MOUSEMOVE,
};

const VK_LBUTTON: u16 = 0x01;
const VK_RBUTTON: u16 = 0x02;
const VK_1: u16 = 0x31;
const VK_5: u16 = 0x35;

/// Virtual input -> the host's mouse button or key.
const NATIVES: &[(&str, u16)] = &[("trigger", VK_LBUTTON), ("bomb", VK_RBUTTON), ("start", VK_1), ("coin", VK_5)];
const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("b2", "bomb"), ("start", "start"), ("coin", "coin")];

fn keymap() -> &'static ButtonMap<u16> {
    static MAP: OnceLock<ButtonMap<u16>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The buttons and keys held (bit n: virtual key n), for `GetAsyncKeyState`.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

/// The host's visible window titled "Veggie..." (its WPF intro window is another).
fn host_window() -> HWND {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != unsafe { GetCurrentProcessId() } || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut title = [0u16; 64];
        let n = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) }.max(0) as usize;
        if String::from_utf16_lossy(&title[..n]).starts_with("Veggie") {
            unsafe { *(found as *mut HWND) = hwnd };
            return 0;
        }
        1
    }
    let mut found: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
    found
}

fn input_thread() {
    let mut key_was = [0u64; 4];
    let mut last = (i32::MIN, i32::MIN);
    let mut hwnd: HWND = std::ptr::null_mut();
    loop {
        std::thread::sleep(Duration::from_millis(8));
        if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 {
            hwnd = host_window();
            if hwnd.is_null() {
                continue;
            }
            log!("veggieblast: host window {hwnd:?}");
        }
        let stick = input(0);

        // the aim: the cursor on the screen
        let (sx, sy) = gun::on_screen(&stick, gun::screen_size());
        let at = (sx as i32, sy as i32);
        set_cursor(at.0, at.1);
        if at != last {
            let mut p = POINT { x: at.0, y: at.1 };
            unsafe { ScreenToClient(hwnd, &mut p) };
            post_mouse(hwnd, WM_MOUSEMOVE, p.x, p.y, 0);
            last = at;
        }

        // the buttons (polled) and the keys (messages)
        let mut keys = [0u64; 4];
        keymap().for_each_pressed(&stick, |vk| keys[vk as usize / 64] |= 1 << (vk % 64));
        for (held, k) in HELD.iter().zip(keys) {
            held.store(k, Ordering::Relaxed);
        }
        for vk in [VK_1, VK_5] {
            let (now, before) = (keys[0] >> vk & 1 != 0, key_was[0] >> vk & 1 != 0);
            if now != before {
                post_key(hwnd, vk, now);
            }
        }
        key_was = keys;
    }
}

/// The host's settings, next to it.
fn settings_path() -> std::path::PathBuf {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("LocalControls.ini"))).unwrap_or_else(|| "LocalControls.ini".into())
}

fn flag(on: bool) -> &'static str {
    if on { "True" } else { "False" }
}

/// `LocalControls.ini` with free play and the easy mode (when given), and the absolute aim.
fn with_settings(data: &str, free_play: Option<bool>, easy: Option<bool>) -> String {
    let mut out = String::new();
    let (mut seen_free, mut seen_easy, mut seen_relative) = (false, false, false);
    for line in data.lines() {
        let key = line.split('=').next().unwrap_or("").trim();
        match key {
            "FreePlay" if free_play.is_some() => {
                seen_free = true;
                out += &format!("FreePlay={}\r\n", flag(free_play == Some(true)));
            }
            "Easy" if easy.is_some() => {
                seen_easy = true;
                out += &format!("Easy={}\r\n", flag(easy == Some(true)));
            }
            "Relative" => {
                seen_relative = true;
                out += "Relative=False\r\n";
            }
            _ if line.trim().is_empty() => {}
            _ => out += &format!("{line}\r\n"),
        }
    }
    if let (false, Some(on)) = (seen_free, free_play) {
        out += &format!("FreePlay={}\r\n", flag(on));
    }
    if let (false, Some(on)) = (seen_easy, easy) {
        out += &format!("Easy={}\r\n", flag(on));
    }
    if !seen_relative {
        out += "Relative=False\r\n";
    }
    out
}

fn settings() {
    let path = settings_path();
    let data = std::fs::read_to_string(&path).unwrap_or_default();
    let easy = std::env::var("WAL_UNIS_VEGGIEBLAST_EASY").ok().map(|v| matches!(v.trim(), "1" | "true" | "on"));
    let new = with_settings(&data, wal_payload_common::free_play("WAL_UNIS_FREEPLAY"), easy);
    if new != data {
        match std::fs::write(&path, &new) {
            Ok(()) => log!("veggieblast: {}: {}", path.display(), new.replace("\r\n", " ")),
            Err(e) => log!("veggieblast: {} not written: {e}", path.display()),
        }
    }
}

pub fn init() {
    settings();
    own_process_input();
    std::thread::spawn(input_thread);
    log!("veggieblast: the host's cursor, mouse buttons and keys played by player 1's stick");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_free_play_and_absolute_aim() {
        let dump = "FreePlay=True\r\nEasy=True\r\nRelative=True\r\nSensitivity=1\r\n";
        assert_eq!(with_settings(dump, Some(false), Some(false)), "FreePlay=False\r\nEasy=False\r\nRelative=False\r\nSensitivity=1\r\n");
        assert_eq!(with_settings(dump, None, None), "FreePlay=True\r\nEasy=True\r\nRelative=False\r\nSensitivity=1\r\n");
        assert_eq!(with_settings("", Some(true), Some(true)), "FreePlay=True\r\nEasy=True\r\nRelative=False\r\n");
    }
}
