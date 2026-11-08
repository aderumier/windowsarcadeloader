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
//!   Unity 5 and older): keyboard and mouse button messages become `WM_NULL`, but the payload's
//!   own keys ([`post_key`], marked in their `lParam`'s reserved bits): the keys a game's own
//!   code reads through Unity's `Input.GetKey` (the DemulShooter plugins' coin and operator keys).
//!   The payload's own mouse messages ([`post_mouse`], marked in their `wParam`'s high bits) go
//!   through too: a game played with the mouse (Bullseye's keyboard mode) aims and shoots with the
//!   gun; [`own_mouse`] also drops the real mouse's moves and answers the Unity player's
//!   `GetCursorPos` with the payload's cursor (Unity reads `Input.mousePosition` there).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_KEYFIRST, WM_KEYLAST, WM_KEYUP, WM_LBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_NULL};

type P = *mut c_void;

static GET_RAW_INPUT_DATA: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_W: AtomicUsize = AtomicUsize::new(0);
static GET_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static GET_MESSAGE_W: AtomicUsize = AtomicUsize::new(0);
static KEY_HELD: OnceLock<fn(i32) -> bool> = OnceLock::new();
/// The real mouse's moves dropped too ([`own_mouse`]).
static OWN_MOUSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
/// `lParam` reserved bits (25..28) of the key messages [`post_key`] posts.
const MARK: isize = 0x0A00_0000;
/// `wParam` bits above the button flags of the mouse messages [`post_mouse`] posts.
const MOUSE_MARK: usize = 0x0A00_0000;
/// `wParam` low word bits above the key flags of the wheel messages [`post_wheel`] posts (its
/// high word is the wheel's delta).
const WHEEL_MARK: usize = 0x0A00;

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
    messages(unity);
}

/// The keyboard and mouse messages of one more module's `PeekMessage` / `GetMessage` calls
/// filtered too (after [`init`]): a second message loop (an MFC program's, in `mfc100u.dll`
/// and in its executable).
pub fn messages(module: usize) {
    if module == 0 {
        return;
    }
    let messages: [(&str, usize, &AtomicUsize); 4] = [
        ("PeekMessageA", peek_message_a as *const () as usize, &PEEK_MESSAGE_A),
        ("PeekMessageW", peek_message_w as *const () as usize, &PEEK_MESSAGE_W),
        ("GetMessageA", get_message_a as *const () as usize, &GET_MESSAGE_A),
        ("GetMessageW", get_message_w as *const () as usize, &GET_MESSAGE_W),
    ];
    for (name, hook, orig) in messages {
        if let Some(o) = unsafe { iat::hook_module(module, "user32.dll", name, hook) } {
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
    if let Some(flags) = own_raw_buttons(raw) {
        return unsafe { own_raw_mouse(flags, command, data, size) };
    }
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

/// A real keyboard or mouse button message (or mouse move, with [`own_mouse`]).
fn filtered(msg: &MSG) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE;
    let key = (WM_KEYFIRST..=WM_KEYLAST).contains(&msg.message);
    let ours = if msg.message == WM_MOUSEWHEEL {
        msg.wParam & 0xFF00 == WHEEL_MARK
    } else {
        msg.wParam & MOUSE_MARK == MOUSE_MARK
    };
    let mouse = (WM_LBUTTONDOWN..=WM_MOUSEHWHEEL).contains(&msg.message)
        || msg.message == WM_MOUSEMOVE && OWN_MOUSE.load(Ordering::Relaxed);
    (key && msg.lParam & MARK != MARK) || (mouse && !ours)
}

/// The cursor's place on the screen (x << 32 | y), the payload's mouse ([`post_mouse`]).
static CURSOR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

unsafe extern "system" fn get_cursor_pos(point: *mut windows_sys::Win32::Foundation::POINT) -> i32 {
    let c = CURSOR.load(Ordering::Relaxed);
    if !point.is_null() {
        unsafe { *point = windows_sys::Win32::Foundation::POINT { x: (c >> 32) as i32, y: c as u32 as i32 } };
    }
    1
}

unsafe extern "system" fn set_cursor_pos(_x: i32, _y: i32) -> i32 {
    1
}

/// The game's mouse is the payload's only ([`post_mouse`]): the real mouse's moves dropped too,
/// and the Unity player's (`unity`) cursor polls (`GetCursorPos`) answer the payload's cursor.
pub fn own_mouse(unity: usize) {
    OWN_MOUSE.store(true, Ordering::Relaxed);
    if unity != 0 {
        unsafe {
            iat::hook_module(unity, "user32.dll", "GetCursorPos", get_cursor_pos as *const () as usize);
            iat::hook_module(unity, "user32.dll", "SetCursorPos", set_cursor_pos as *const () as usize);
        }
    }
}

/// Posts a mouse message (`WM_MOUSEMOVE`, `WM_LBUTTONDOWN`...) at client point (`x`, `y`) of the
/// game window `hwnd`, `buttons` its `MK_` flags: the filter lets it through.
pub fn post_mouse(hwnd: P, message: u32, x: i32, y: i32, buttons: usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    let lparam = ((y as u16 as isize) << 16) | x as u16 as isize;
    let mut at = POINT { x, y };
    unsafe { ClientToScreen(hwnd, &mut at) };
    CURSOR.store((at.x as u32 as u64) << 32 | at.y as u32 as u64, Ordering::Relaxed);
    unsafe { PostMessageW(hwnd, message, buttons | MOUSE_MARK, lparam) };
}

/// Posts a mouse wheel turn of `delta` (120 a notch, positive away from the player) at client
/// point (`x`, `y`) of the game window `hwnd`: the filter lets it through.
pub fn post_wheel(hwnd: P, delta: i16, x: i32, y: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    let mut at = POINT { x, y };
    unsafe { ClientToScreen(hwnd, &mut at) };
    let lparam = ((at.y as u16 as isize) << 16) | at.x as u16 as isize;
    unsafe { PostMessageW(hwnd, WM_MOUSEWHEEL, (delta as u16 as usize) << 16 | WHEEL_MARK, lparam) };
}

unsafe fn scrub(msg: *mut MSG) {
    if !msg.is_null() && unsafe { filtered(&*msg) } {
        unsafe { (*msg).message = WM_NULL };
    }
}

/// The raw mouse reports [`post_raw_buttons`] posted: their `HRAWINPUT` handles (`RAW_MARK` and
/// a sequence number), their button flags.
const RAW_MARK: usize = 0x57A1_0000;
const RAW_SLOTS: usize = 64;
static RAW_SEQ: AtomicUsize = AtomicUsize::new(0);
static RAW_FLAGS: [AtomicUsize; RAW_SLOTS] = [const { AtomicUsize::new(0) }; RAW_SLOTS];
/// `GetRawInputData` command returning the header only.
const RID_HEADER: u32 = 0x1000_0005;

fn own_raw_buttons(raw: P) -> Option<u16> {
    let h = raw as usize;
    (h & !0xFFFF == RAW_MARK).then(|| RAW_FLAGS[h % RAW_SLOTS].load(Ordering::Relaxed) as u16)
}

/// The first raw mouse of the system (a report names its device; 0: none).
fn raw_mouse_device() -> usize {
    use windows_sys::Win32::UI::Input::{GetRawInputDeviceList, RAWINPUTDEVICELIST};
    static DEVICE: OnceLock<usize> = OnceLock::new();
    *DEVICE.get_or_init(|| {
        let mut list = [RAWINPUTDEVICELIST { hDevice: std::ptr::null_mut(), dwType: 0 }; 32];
        let mut n = list.len() as u32;
        let got = unsafe { GetRawInputDeviceList(list.as_mut_ptr(), &mut n, size_of::<RAWINPUTDEVICELIST>() as u32) };
        if got == u32::MAX {
            return 0;
        }
        list[..got as usize].iter().find(|d| d.dwType == RIM_TYPEMOUSE).map_or(0, |d| d.hDevice as usize)
    })
}

/// The answer to `GetRawInputData` for a report of the payload's: a mouse report of `flags`
/// (`RI_MOUSE_*` button transitions), no motion.
unsafe fn own_raw_mouse(flags: u16, command: u32, data: *mut u8, size: *mut u32) -> u32 {
    let full = if command == RID_HEADER { HEADER } else { HEADER + RAWMOUSE };
    if size.is_null() {
        return u32::MAX;
    }
    if data.is_null() {
        unsafe { *size = full as u32 };
        return 0;
    }
    if (unsafe { *size } as usize) < full {
        return u32::MAX;
    }
    let mut report = [0u8; HEADER + RAWMOUSE];
    report[..4].copy_from_slice(&RIM_TYPEMOUSE.to_le_bytes());
    report[4..8].copy_from_slice(&((HEADER + RAWMOUSE) as u32).to_le_bytes());
    report[8..8 + size_of::<usize>()].copy_from_slice(&raw_mouse_device().to_le_bytes());
    // RAWMOUSE: usFlags 0 (relative), usButtonFlags at +4
    report[HEADER + 4..HEADER + 6].copy_from_slice(&flags.to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(report.as_ptr(), data, full) };
    full as u32
}

/// Posts a raw mouse report (`WM_INPUT`) of button transitions `flags` (`RI_MOUSE_LEFT_BUTTON_DOWN`
/// 1, `_UP` 2...) to the game window `hwnd`: games reading the mouse's buttons from raw input
/// (Unity 4) get the payload's.
pub fn post_raw_buttons(hwnd: P, flags: u16) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_INPUT};
    let seq = RAW_SEQ.fetch_add(1, Ordering::Relaxed) % RAW_SLOTS;
    RAW_FLAGS[seq].store(flags as usize, Ordering::Relaxed);
    // RIM_INPUT (the window in the foreground)
    unsafe { PostMessageW(hwnd, WM_INPUT, 0, (RAW_MARK | seq) as isize) };
}

/// Posts a key press (`down`) or release of virtual key `vk` to the game window `hwnd`, as the
/// keyboard would: the filter lets it through. `key_held` should report it held meanwhile.
pub fn post_key(hwnd: P, vk: u16, down: bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_KEYDOWN};
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as isize & 0xFF;
    // the arrows, Insert/Delete, Home/End, Page Up/Down are extended keys
    let extended = if (0x21..=0x2E).contains(&vk) { 1 << 24 } else { 0 };
    let up = if down { 0 } else { 0xC000_0000u32 as i32 as isize };
    let lparam = 1 | scan << 16 | extended | MARK | up;
    unsafe { PostMessageW(hwnd, if down { WM_KEYDOWN } else { WM_KEYUP }, vk as usize, lparam) };
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
