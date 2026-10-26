//! The real keyboard and mouse hidden from a Unity game; the keys it polls from the virtual
//! sticks.
//!
//! Unity reads the keyboard and mouse through its window messages and raw input; a game's own
//! code may also poll `GetAsyncKeyState` (Galaxy Rangers' `ArcadeControls`). Wine's user32 calls
//! win32u for these (`GetAsyncKeyState` is a jump through user32's import of
//! `NtUserGetAsyncKeyState`): user32's imports are hooked, so every caller, Unity's and the Mono
//! `DllImport`s alike, gets ours:
//!
//! * `GetAsyncKeyState`, `GetKeyState`, `GetKeyboardState`: only the keys the payload's
//!   `key_held` reports (operator keys from the virtual sticks) are down.
//! * `GetRawInputData`: keyboard reports become the release of no key (virtual key 0xFF), mouse
//!   reports a motion of 0 without buttons.
//! * The Unity player's `PeekMessage` / `GetMessage` (`UnityPlayer.dll`, or the executable of
//!   Unity 5 and older): keyboard and mouse button messages become `WM_NULL`.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_KEYFIRST, WM_KEYLAST, WM_KEYUP, WM_LBUTTONDOWN, WM_MOUSEHWHEEL, WM_NULL};

type P = *mut c_void;

static GET_RAW_INPUT_DATA: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_W: AtomicUsize = AtomicUsize::new(0);
static GET_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static GET_MESSAGE_W: AtomicUsize = AtomicUsize::new(0);
static KEY_HELD: OnceLock<fn(i32) -> bool> = OnceLock::new();

/// `GetRawInputData` command returning the whole report.
const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;
/// RAWINPUTHEADER: type, size, device, wParam.
const HEADER: usize = 8 + 2 * size_of::<usize>();
/// RAWKEYBOARD: make code, flags, reserved, virtual key, message, extra.
const RAWKEYBOARD: usize = 16;
/// RAWMOUSE: flags, button flags and data, raw buttons, last x, last y, extra.
const RAWMOUSE: usize = 24;

/// A virtual key reported down.
fn key_held(vk: i32) -> bool {
    KEY_HELD.get().is_some_and(|f| f(vk))
}

/// Installs the filters. `unity`: the module importing the Unity player's message functions
/// (`UnityPlayer.dll`, or the executable of Unity 5 and older; 0: none). `key_held`: whether a
/// virtual key is down for the game.
pub fn init(unity: usize, key_held: fn(i32) -> bool) {
    let _ = KEY_HELD.set(key_held);
    let user32 = unsafe { GetModuleHandleW(windows_sys::w!("user32.dll")) as usize };
    let win32u: [(&str, usize, Option<&AtomicUsize>); 4] = [
        ("NtUserGetAsyncKeyState", get_async_key_state as *const () as usize, None),
        ("NtUserGetKeyState", get_key_state as *const () as usize, None),
        ("NtUserGetKeyboardState", get_keyboard_state as *const () as usize, None),
        ("NtUserGetRawInputData", get_raw_input_data as *const () as usize, Some(&GET_RAW_INPUT_DATA)),
    ];
    for (name, hook, orig) in win32u {
        match unsafe { iat::hook_module(user32, "win32u.dll", name, hook) } {
            Some(o) => {
                if let Some(slot) = orig {
                    slot.store(o, Ordering::Relaxed);
                }
            }
            None => log!("filter: user32 does not import win32u!{name}: the real keyboard may reach the game"),
        }
    }
    if unity == 0 {
        return;
    }
    let messages: [(&str, usize, &AtomicUsize); 4] = [
        ("PeekMessageA", peek_message_a as *const () as usize, &PEEK_MESSAGE_A),
        ("PeekMessageW", peek_message_w as *const () as usize, &PEEK_MESSAGE_W),
        ("GetMessageA", get_message_a as *const () as usize, &GET_MESSAGE_A),
        ("GetMessageW", get_message_w as *const () as usize, &GET_MESSAGE_W),
    ];
    for (name, hook, orig) in messages {
        if let Some(o) = unsafe { iat::hook_module(unity, "user32.dll", name, hook) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
}

unsafe extern "system" fn get_async_key_state(vk: i32) -> i16 {
    if key_held(vk) { i16::MIN } else { 0 }
}

unsafe extern "system" fn get_key_state(vk: i32) -> i16 {
    if key_held(vk) { i16::MIN } else { 0 }
}

unsafe extern "system" fn get_keyboard_state(keys: *mut u8) -> i32 {
    if keys.is_null() {
        return 0;
    }
    for vk in 0..256 {
        unsafe { *keys.add(vk) = if key_held(vk as i32) { 0x80 } else { 0 } };
    }
    1
}

unsafe extern "system" fn get_raw_input_data(raw: P, command: u32, data: *mut u8, size: *mut u32, header_size: u32) -> u32 {
    let f: unsafe extern "system" fn(P, u32, *mut u8, *mut u32, u32) -> u32 =
        unsafe { std::mem::transmute(GET_RAW_INPUT_DATA.load(Ordering::Relaxed)) };
    let n = unsafe { f(raw, command, data, size, header_size) };
    if command != RID_INPUT || data.is_null() || n == u32::MAX || (n as usize) < HEADER {
        return n;
    }
    unsafe {
        let body = data.add(HEADER);
        match (data as *const u32).read_unaligned() {
            RIM_TYPEKEYBOARD if n as usize >= HEADER + RAWKEYBOARD => {
                // make code 0, RI_KEY_BREAK, virtual key 0xFF, WM_KEYUP
                (body as *mut u16).write_unaligned(0);
                (body.add(2) as *mut u16).write_unaligned(1);
                (body.add(6) as *mut u16).write_unaligned(0xFF);
                (body.add(8) as *mut u32).write_unaligned(WM_KEYUP);
            }
            RIM_TYPEMOUSE if n as usize >= HEADER + RAWMOUSE => {
                // relative motion, no buttons, no wheel, no motion
                std::ptr::write_bytes(body, 0, RAWMOUSE - 4);
            }
            _ => {}
        }
    }
    n
}

/// A real keyboard or mouse button message.
fn filtered(msg: u32) -> bool {
    (WM_KEYFIRST..=WM_KEYLAST).contains(&msg) || (WM_LBUTTONDOWN..=WM_MOUSEHWHEEL).contains(&msg)
}

unsafe fn scrub(msg: *mut MSG) {
    if !msg.is_null() && unsafe { filtered((*msg).message) } {
        unsafe { (*msg).message = WM_NULL };
    }
}

type PeekMessage = unsafe extern "system" fn(*mut MSG, P, u32, u32, u32) -> i32;
type GetMessage = unsafe extern "system" fn(*mut MSG, P, u32, u32) -> i32;

unsafe fn peek(orig: &AtomicUsize, msg: *mut MSG, hwnd: P, min: u32, max: u32, remove: u32) -> i32 {
    let f: PeekMessage = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { f(msg, hwnd, min, max, remove) };
    if r != 0 {
        unsafe { scrub(msg) };
    }
    r
}

unsafe fn get(orig: &AtomicUsize, msg: *mut MSG, hwnd: P, min: u32, max: u32) -> i32 {
    let f: GetMessage = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { f(msg, hwnd, min, max) };
    if r > 0 {
        unsafe { scrub(msg) };
    }
    r
}

unsafe extern "system" fn peek_message_a(msg: *mut MSG, hwnd: P, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek(&PEEK_MESSAGE_A, msg, hwnd, min, max, remove) }
}

unsafe extern "system" fn peek_message_w(msg: *mut MSG, hwnd: P, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek(&PEEK_MESSAGE_W, msg, hwnd, min, max, remove) }
}

unsafe extern "system" fn get_message_a(msg: *mut MSG, hwnd: P, min: u32, max: u32) -> i32 {
    unsafe { get(&GET_MESSAGE_A, msg, hwnd, min, max) }
}

unsafe extern "system" fn get_message_w(msg: *mut MSG, hwnd: P, min: u32, max: u32) -> i32 {
    unsafe { get(&GET_MESSAGE_W, msg, hwnd, min, max) }
}
