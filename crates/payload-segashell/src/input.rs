//! The game's PC inputs hidden: its input comes from the launcher only (the I/O board, a
//! DirectInput game controller, from the virtual sticks).
//!
//! Showdown (a PC game made arcade) still reads the keyboard and the mouse (window messages,
//! raw input, low level hooks) and pads (XInput): its `PeekMessageW` / `A` drop the keyboard,
//! mouse and raw input messages, `RegisterRawInputDevices` registers nothing, the low level
//! keyboard / mouse hooks are not installed (a handle the game can unhook), XInput reports no
//! pad.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MSG, PM_REMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_INPUT, WM_KEYFIRST, WM_KEYLAST, WM_MOUSEFIRST, WM_MOUSELAST,
};

type PeekMessage = unsafe extern "system" fn(*mut MSG, usize, u32, u32, u32) -> i32;
type SetWindowsHookEx = unsafe extern "system" fn(i32, usize, usize, u32) -> usize;

static PEEK_W: AtomicUsize = AtomicUsize::new(0);
static PEEK_A: AtomicUsize = AtomicUsize::new(0);
static SET_HOOK: AtomicUsize = AtomicUsize::new(0);

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

pub(crate) fn init() {
    let hooks: [(&str, &AtomicUsize, usize); 3] = [
        ("PeekMessageW", &PEEK_W, peek_message_w as *const () as usize),
        ("PeekMessageA", &PEEK_A, peek_message_a as *const () as usize),
        ("SetWindowsHookExA", &SET_HOOK, set_windows_hook as *const () as usize),
    ];
    for (name, orig, hook) in hooks {
        if let Some(o) = unsafe { iat::hook("user32.dll", name, hook) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
    unsafe { iat::hook("user32.dll", "RegisterRawInputDevices", register_raw_input as *const () as usize) };
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let xinput: [(u16, usize); 3] = [
        (2, xinput_get_state as *const () as usize),
        (3, xinput_set_state as *const () as usize),
        (4, xinput_get_capabilities as *const () as usize),
    ];
    for (ordinal, f) in xinput {
        unsafe { iat::hook_ordinal(exe, "xinput1_3.dll", ordinal, f) };
    }
}

fn is_pc_input(msg: u32) -> bool {
    (WM_KEYFIRST..=WM_KEYLAST).contains(&msg) || (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&msg) || msg == WM_INPUT
}

unsafe extern "system" fn peek_message_w(msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek_message(&PEEK_W, msg, hwnd, min, max, remove) }
}

unsafe extern "system" fn peek_message_a(msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek_message(&PEEK_A, msg, hwnd, min, max, remove) }
}

/// The game's `PeekMessage` (`orig`): the PC's input messages removed, the others returned.
unsafe fn peek_message(orig: &AtomicUsize, msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    let peek: PeekMessage = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    loop {
        let r = unsafe { peek(msg, hwnd, min, max, remove) };
        if r == 0 || msg.is_null() || !is_pc_input(unsafe { (*msg).message }) {
            return r;
        }
        if remove & PM_REMOVE == 0 {
            // only looking: take that input message out, then look again
            let mut drop: MSG = unsafe { std::mem::zeroed() };
            let m = unsafe { (*msg).message };
            unsafe { peek(&mut drop, hwnd, m, m, PM_REMOVE) };
        }
    }
}

unsafe extern "system" fn register_raw_input(_devices: usize, count: u32, _size: u32) -> i32 {
    log!("input: raw input ({count} device(s)) not registered");
    1
}

/// Low level keyboard / mouse hooks: not installed (a handle that unhooks nothing).
unsafe extern "system" fn set_windows_hook(id: i32, proc: usize, module: usize, thread: u32) -> usize {
    if id == WH_KEYBOARD_LL || id == WH_MOUSE_LL {
        log!("input: low level hook {id} not installed");
        return 0x5741_4c00 + id as usize;
    }
    let f: SetWindowsHookEx = unsafe { std::mem::transmute(SET_HOOK.load(Ordering::Relaxed)) };
    unsafe { f(id, proc, module, thread) }
}

extern "system" fn xinput_get_state(_user: u32, _state: usize) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

extern "system" fn xinput_set_state(_user: u32, _vibration: usize) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

extern "system" fn xinput_get_capabilities(_user: u32, _flags: u32, _caps: usize) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}
