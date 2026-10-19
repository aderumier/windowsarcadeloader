//! The game's own PC inputs, fed from the virtual sticks only.
//!
//! Besides the board, the games read the keyboard from their window messages (`PeekMessageW`,
//! Storm Rider `PeekMessageA`; Storm Racer G: Enter starts, Space inserts a coin, arrows /
//! Q W E B / Backspace in menus and races) and pads through XInput. Every input must come from the launcher: the game's `PeekMessage` drops
//! the real keyboard messages and returns key messages of its own instead (natives `key_*`,
//! pressed and released with their virtual stick inputs); `XInputGetState` reports no pad.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, WM_KEYDOWN, WM_KEYFIRST, WM_KEYLAST, WM_KEYUP};

type PeekMessage = unsafe extern "system" fn(*mut MSG, usize, u32, u32, u32) -> i32;

static PEEK_W: AtomicUsize = AtomicUsize::new(0);
static PEEK_A: AtomicUsize = AtomicUsize::new(0);
/// The game window (seen in its messages): where our key messages go.
static WINDOW: AtomicUsize = AtomicUsize::new(0);
/// Key messages to hand to the game: (message, virtual key).
static PENDING: Mutex<VecDeque<(u32, u32)>> = Mutex::new(VecDeque::new());

pub(crate) fn init() {
    let hooks = [("PeekMessageW", &PEEK_W, peek_message_w as *const () as usize), ("PeekMessageA", &PEEK_A, peek_message_a as *const () as usize)];
    for (name, orig, hook) in hooks {
        if let Some(o) = unsafe { iat::hook("user32.dll", name, hook) } {
            orig.store(o, Ordering::Relaxed);
            log!("keys: {name}: game keyboard from the virtual sticks only");
        }
    }
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    if unsafe { iat::hook_ordinal(base, "xinput1_3.dll", 2, xinput_get_state as *const () as usize) }.is_some() {
        log!("keys: XInput pads hidden");
    }
}

/// A virtual key pressed (`down`) or released.
pub(crate) fn key(vk: u8, down: bool) {
    let msg = if down { WM_KEYDOWN } else { WM_KEYUP };
    PENDING.lock().unwrap().push_back((msg, vk as u32));
}

fn is_key_message(msg: u32) -> bool {
    (WM_KEYFIRST..=WM_KEYLAST).contains(&msg)
}

unsafe extern "system" fn peek_message_w(msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek_message(&PEEK_W, msg, hwnd, min, max, remove) }
}

unsafe extern "system" fn peek_message_a(msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek_message(&PEEK_A, msg, hwnd, min, max, remove) }
}

/// The game's `PeekMessageW` / `PeekMessageA` (`orig`): key messages from the virtual sticks only.
unsafe fn peek_message(orig: &AtomicUsize, msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    let peek: PeekMessage = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    // the window the game polls (Storm Rider: its own), else the one seen in its messages
    if hwnd != 0 {
        WINDOW.store(hwnd, Ordering::Relaxed);
    }
    let window = WINDOW.load(Ordering::Relaxed);
    let wants_keys = (min == 0 && max == 0) || (min <= WM_KEYDOWN && WM_KEYUP <= max);
    if window != 0 && wants_keys {
        let mut pending = PENDING.lock().unwrap();
        if let Some(&(message, vk)) = pending.front() {
            if remove & PM_REMOVE != 0 {
                pending.pop_front();
            }
            let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) };
            let up = if message == WM_KEYUP { 0xC000_0000 } else { 0 };
            unsafe {
                *msg = std::mem::zeroed();
                (*msg).hwnd = window as _;
                (*msg).message = message;
                (*msg).wParam = vk as usize;
                (*msg).lParam = (1 | scan << 16 | up) as isize;
            }
            return 1;
        }
    }
    loop {
        let r = unsafe { peek(msg, hwnd, min, max, remove) };
        if r == 0 {
            return 0;
        }
        let (message, from) = unsafe { ((*msg).message, (*msg).hwnd as usize) };
        if from != 0 && window == 0 {
            WINDOW.store(from, Ordering::Relaxed);
        }
        if !is_key_message(message) {
            return r;
        }
        // a real key: dropped (removed from the queue when the game only peeked)
        if remove & PM_REMOVE == 0 {
            unsafe { peek(msg, hwnd, message, message, PM_REMOVE) };
        }
    }
}

/// `XInputGetState`: ERROR_DEVICE_NOT_CONNECTED.
unsafe extern "system" fn xinput_get_state(_user: u32, _state: *mut u8) -> u32 {
    0x48F
}
