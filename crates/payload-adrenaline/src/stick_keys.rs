//! The players' joysticks as the game's keyboard keys: `WAL_ADRENALINE_STICK_KEYS=1`.
//!
//! Shooty Skies moves each player's ship with Unity's `Input.GetKey` when no XInput pad is
//! configured (`InputManager.GetDirection`: player 1 on W A S D, player 2 on the arrows); its
//! buttons are the Kaboom board's. The real keyboard is filtered (`input_filter`): the
//! payload posts these keys to the game's window from the players' `stick_up`, `stick_down`,
//! `stick_left` and `stick_right`, and reports them held to its key state polls. The game's
//! own debug key handler also turns the up arrow into player 2's button (`BUTTON_DOWN`): only
//! used to join from the attract screen.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::{input_filter, log};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId};

use crate::{Native, held};

/// Each player's keys: up, down, left, right.
const KEYS: [[u16; 4]; 2] = [[b'W' as u16, b'S' as u16, b'A' as u16, b'D' as u16], [0x26, 0x28, 0x25, 0x27]];
const DIRECTIONS: [Native; 4] = [Native::StickUp, Native::StickDown, Native::StickLeft, Native::StickRight];

/// The virtual keys held (a bit each, 256 keys).
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn bit(vk: u16) -> (usize, u64) {
    (vk as usize / 64 % 4, 1 << (vk % 64))
}

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_ADRENALINE_STICK_KEYS").is_ok_and(|v| v.trim() == "1"))
}

/// `input_filter`'s key state: the keys posted held.
pub fn key_held(vk: i32) -> bool {
    let (word, bit) = bit((vk & 0xFF) as u16);
    HELD[word].load(Ordering::Relaxed) & bit != 0
}

/// The Unity player's window (`UnityWndClass`) of this process.
fn unity_window() -> HWND {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let mut class = [0u16; 32];
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } as usize;
        if pid == unsafe { GetCurrentProcessId() } && String::from_utf16_lossy(&class[..n]) == "UnityWndClass" {
            unsafe { *(found as *mut HWND) = hwnd };
            return 0;
        }
        1
    }
    let mut found: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
    found
}

/// Posts the keys of the players' joysticks every 10 ms.
pub fn init() {
    if !enabled() {
        return;
    }
    log!("stick keys: player 1 on W A S D, player 2 on the arrows");
    std::thread::spawn(|| {
        let mut window: HWND = std::ptr::null_mut();
        loop {
            std::thread::sleep(Duration::from_millis(10));
            if window.is_null() {
                window = unity_window();
                continue;
            }
            for (player, keys) in KEYS.iter().enumerate() {
                for (&vk, &native) in keys.iter().zip(&DIRECTIONS) {
                    let (was, is) = (key_held(vk as i32), held(player, native));
                    if was == is {
                        continue;
                    }
                    // held first, so the game's key state polling agrees with the message
                    let (word, bit) = bit(vk);
                    if is {
                        HELD[word].fetch_or(bit, Ordering::Relaxed);
                    }
                    input_filter::post_key(window, vk, is);
                    if !is {
                        HELD[word].fetch_and(!bit, Ordering::Relaxed);
                    }
                }
            }
        }
    });
}
