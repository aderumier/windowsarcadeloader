//! Rainbow Bomber Girl (`game.exe`, Unity 2017.2, Mono, 64-bit, Direct3D 11). The cabinet's
//! launcher (BM-System) is absent from the dump: the game runs as Unity started it, with its
//! development stand-ins for the system library, its I/O board on a serial port (COM6) and its
//! touch screen fed by nothing. The payload is installed as `winhttp.dll` beside the executable
//! (UnityPlayer imports it: loaded before any of the game's code; its one import is forwarded
//! to the system's) and answers:
//!
//! * the system library's System, Io and Consume modules (`bbg_system.rs`: boot checks,
//!   watchdog, test / service / coin switches, credits), bound to the game through Mono;
//! * the I/O board (`bbg_io.rs`): its status handed to the game's own board manager (UsbIO's
//!   `IoManagerMain`), the serial port itself absent; the touch screen's touches;
//! * the cabinet's two monitors (`bbg_screen.rs`): the game draws a portrait main monitor and
//!   the landscape touch screen turned on its side in one picture, shown side by side upright.
//!
//! The real keyboard and mouse never reach the game (its development keys: Unity's keyboard,
//! window messages and raw input dropped).
//!
//! Native inputs for `native_map`: `test service coin bomb skill1 skill2 back detonate
//! headphone up down left right touch`.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_KEYFIRST, WM_KEYLAST, WM_MOUSEFIRST, WM_MOUSELAST, WM_NULL};

type P = *mut c_void;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Native {
    Test,
    Service,
    Coin,
    /// Puts a bomb (the left, center and right buttons of the first row).
    Bomb,
    /// The first skill (second row).
    Skill1,
    /// The second skill (third row).
    Skill2,
    /// The stick's button: back in the menus.
    Back,
    /// The push button: remote detonation.
    Detonate,
    /// Headphones plugged in.
    Headphone,
    Up,
    Down,
    Left,
    Right,
    /// A touch of the touch screen (the pointer's position).
    Touch,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("bomb", Native::Bomb),
    ("skill1", Native::Skill1),
    ("skill2", Native::Skill2),
    ("back", Native::Back),
    ("detonate", Native::Detonate),
    ("headphone", Native::Headphone),
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
    ("touch", Native::Touch),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "bomb"),
    ("b2", "skill1"),
    ("b3", "skill2"),
    ("b4", "back"),
    ("b5", "detonate"),
    ("b7", "touch"),
    ("start", "bomb"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

pub(crate) fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// Player 1's pressed natives.
pub(crate) fn pressed() -> Vec<Native> {
    let mut v = Vec::new();
    map().for_each_pressed(&wal_payload_common::input(0), |n| v.push(n));
    v
}

/// This copy is the game's `winhttp.dll`, beside Rainbow Bomber Girl's `game.exe` (its UsbIO
/// assembly).
pub fn detected(module: crate::HINSTANCE) -> bool {
    if !crate::file_name(module).eq_ignore_ascii_case("winhttp.dll") {
        return false;
    }
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    exe.with_file_name("game_data").join("managed").join("usbio.dll").is_file()
}

pub fn init() {
    wal_payload_common::start("konami");
    filter_input();
    windowed_resolution();
    crate::bbg_io::init();
    crate::bbg_screen::init();
    log!("konami: BM-System, I/O board and touch screen emulated (Rainbow Bomber Girl)");
}

// ---- winhttp.dll ----

static WINHTTP_PROXY: AtomicUsize = AtomicUsize::new(0);

/// UnityPlayer's one winhttp import, forwarded to the system's winhttp.dll.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn WinHttpGetIEProxyConfigForCurrentUser(config: P) -> i32 {
    let mut f = WINHTTP_PROXY.load(Ordering::Relaxed);
    if f == 0 {
        let mut dir = [0u16; 260];
        let n = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
        let path: Vec<u16> = dir[..n].iter().copied().chain("\\winhttp.dll".encode_utf16()).chain([0]).collect();
        let module = unsafe { LoadLibraryW(path.as_ptr()) };
        f = unsafe { GetProcAddress(module, c"WinHttpGetIEProxyConfigForCurrentUser".as_ptr() as *const u8) }.map_or(0, |f| f as usize);
        WINHTTP_PROXY.store(f, Ordering::Relaxed);
    }
    if f == 0 {
        return 0;
    }
    let real: unsafe extern "system" fn(P) -> i32 = unsafe { std::mem::transmute(f) };
    unsafe { real(config) }
}

// ---- the cabinet's 2160x1920 desktop ----

static UNITY_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ADD_INTERNAL_CALL: AtomicUsize = AtomicUsize::new(0);
static SET_RESOLUTION: AtomicUsize = AtomicUsize::new(0);

type GetProcAddressFn = unsafe extern "system" fn(P, *const u8) -> P;
type AddInternalCall = unsafe extern "C" fn(*const i8, P);
type SetResolution = unsafe extern "C" fn(i32, i32, u8, i32);

/// Unity full screen only takes a screen mode the display has (else the screen's own: the
/// touch screen then no longer fits its picture). The game runs windowed instead (`-screen-
/// fullscreen 0`, its window the screen's size: `WAL_WINDOW_SIZE=screen`), its picture drawn at
/// the cabinet's size and stretched to the window: `Screen.SetResolution` (the game asks for
/// full screen) is registered by UnityPlayer through Mono's `mono_add_internal_call`, which
/// it finds with `GetProcAddress`: the payload's wrapper always asks for a window.
fn windowed_resolution() {
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    cabinet_window_size(unity);
    match unsafe { iat::hook_module(unity, "kernel32.dll", "GetProcAddress", unity_get_proc_address as *const () as usize) } {
        Some(o) => UNITY_GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("bbg: UnityPlayer does not import GetProcAddress: full screen at the screen's size"),
    }
}

unsafe extern "system" fn unity_get_proc_address(module: P, name: *const u8) -> P {
    let orig: GetProcAddressFn = unsafe { std::mem::transmute(UNITY_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if !f.is_null() && (name as usize) >> 16 != 0 && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() == b"mono_add_internal_call" {
        ADD_INTERNAL_CALL.store(f as usize, Ordering::Relaxed);
        return add_internal_call as *const () as P;
    }
    f
}

unsafe extern "C" fn add_internal_call(name: *const i8, method: P) {
    let orig: AddInternalCall = unsafe { std::mem::transmute(ADD_INTERNAL_CALL.load(Ordering::Relaxed)) };
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes() == b"UnityEngine.Screen::SetResolution" {
        SET_RESOLUTION.store(method as usize, Ordering::Relaxed);
        return unsafe { orig(name, set_resolution as *const () as P) };
    }
    unsafe { orig(name, method) }
}

static GET_CLIENT_RECT: AtomicUsize = AtomicUsize::new(0);
static GET_WINDOW_RECT: AtomicUsize = AtomicUsize::new(0);

type GetRect = unsafe extern "system" fn(HWND, *mut windows_sys::Win32::Foundation::RECT) -> i32;

/// The cabinet's desktop.
pub(crate) const DESKTOP: (i32, i32) = (2160, 1920);

static REGISTER_CLASS_EX_W: AtomicUsize = AtomicUsize::new(0);
static UNITY_WNDPROC: AtomicUsize = AtomicUsize::new(0);

/// Unity sizes its windowed picture (its buffers, the cameras' layout) from its window's: its
/// window covering the screen (a top-level window the screen's size) reads as 2160x1920, in
/// `GetClientRect`, `GetWindowRect` and the `WM_SIZE` its window procedure (`UnityWndClass`,
/// wrapped at its registration) receives.
fn cabinet_window_size(unity: usize) {
    let hooks: [(&str, usize, &AtomicUsize); 9] = [
        ("SetWindowPos", set_window_pos as *const () as usize, &SET_WINDOW_POS),
        ("GetClientRect", get_client_rect as *const () as usize, &GET_CLIENT_RECT),
        ("GetWindowRect", get_window_rect as *const () as usize, &GET_WINDOW_RECT),
        ("RegisterClassExW", register_class_ex_w as *const () as usize, &REGISTER_CLASS_EX_W),
        ("GetSystemMetrics", get_system_metrics as *const () as usize, &GET_SYSTEM_METRICS),
        ("GetMonitorInfoW", get_monitor_info_w as *const () as usize, &GET_MONITOR_INFO_W),
        ("GetMonitorInfoA", get_monitor_info_a as *const () as usize, &GET_MONITOR_INFO_A),
        ("EnumDisplaySettingsW", enum_display_settings_w as *const () as usize, &ENUM_DISPLAY_SETTINGS_W),
        ("EnumDisplaySettingsA", enum_display_settings_a as *const () as usize, &ENUM_DISPLAY_SETTINGS_A),
    ];
    for (name, hook, orig) in hooks {
        if let Some(o) = unsafe { iat::hook_module(unity, "user32.dll", name, hook) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
}

static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);

/// Unity's window resized to its picture (2160x1920): kept the screen's size at its top left
/// corner (the picture stretched to it).
unsafe extern "system" fn set_window_pos(hwnd: HWND, after: HWND, mut x: i32, mut y: i32, mut cx: i32, mut cy: i32, flags: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetParent, SWP_NOMOVE, SWP_NOSIZE};
    if flags & SWP_NOSIZE == 0 && unsafe { GetParent(hwnd) }.is_null() && (cx, cy) != screen_size() {
        (cx, cy) = screen_size();
        if flags & SWP_NOMOVE == 0 {
            (x, y) = (0, 0);
        }
    }
    let orig: unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(SET_WINDOW_POS.load(Ordering::Relaxed)) };
    unsafe { orig(hwnd, after, x, y, cx, cy, flags) }
}

static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);
static GET_MONITOR_INFO_W: AtomicUsize = AtomicUsize::new(0);
static GET_MONITOR_INFO_A: AtomicUsize = AtomicUsize::new(0);
static ENUM_DISPLAY_SETTINGS_W: AtomicUsize = AtomicUsize::new(0);
static ENUM_DISPLAY_SETTINGS_A: AtomicUsize = AtomicUsize::new(0);

/// The screen's size metrics (screen, virtual screen, full screen client, maximized window).
unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    let orig: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
    match index {
        0 | 16 | 61 | 78 => DESKTOP.0,
        1 | 17 | 62 | 79 => DESKTOP.1,
        _ => unsafe { orig(index) },
    }
}

/// MONITORINFO: cbSize, rcMonitor, rcWork: the cabinet's desktop.
unsafe fn monitor_info(orig: &AtomicUsize, monitor: P, info: *mut i32) -> i32 {
    let orig: unsafe extern "system" fn(P, *mut i32) -> i32 = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { orig(monitor, info) };
    if r != 0 && !info.is_null() {
        for i in [1, 5] {
            let rect = [0, 0, DESKTOP.0, DESKTOP.1];
            unsafe { std::ptr::copy_nonoverlapping(rect.as_ptr(), info.add(i), 4) };
        }
    }
    r
}

unsafe extern "system" fn get_monitor_info_w(monitor: P, info: *mut i32) -> i32 {
    unsafe { monitor_info(&GET_MONITOR_INFO_W, monitor, info) }
}

unsafe extern "system" fn get_monitor_info_a(monitor: P, info: *mut i32) -> i32 {
    unsafe { monitor_info(&GET_MONITOR_INFO_A, monitor, info) }
}

/// The current display mode (ENUM_CURRENT_SETTINGS, ENUM_REGISTRY_SETTINGS): the cabinet's
/// desktop (DEVMODEW / DEVMODEA dmPelsWidth at 172 / 108).
unsafe fn display_settings(orig: &AtomicUsize, size_at: usize, device: P, mode: u32, devmode: *mut u8) -> i32 {
    let orig: unsafe extern "system" fn(P, u32, *mut u8) -> i32 = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { orig(device, mode, devmode) };
    if r != 0 && mode >= 0xffff_fffe && !devmode.is_null() {
        unsafe {
            (devmode.add(size_at) as *mut i32).write_unaligned(DESKTOP.0);
            (devmode.add(size_at + 4) as *mut i32).write_unaligned(DESKTOP.1);
        }
    }
    r
}

unsafe extern "system" fn enum_display_settings_w(device: P, mode: u32, devmode: *mut u8) -> i32 {
    unsafe { display_settings(&ENUM_DISPLAY_SETTINGS_W, 172, device, mode, devmode) }
}

unsafe extern "system" fn enum_display_settings_a(device: P, mode: u32, devmode: *mut u8) -> i32 {
    unsafe { display_settings(&ENUM_DISPLAY_SETTINGS_A, 108, device, mode, devmode) }
}

/// The real screen (the payload's own calls are not hooked).
fn screen_size() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

unsafe extern "system" fn register_class_ex_w(class: *mut windows_sys::Win32::UI::WindowsAndMessaging::WNDCLASSEXW) -> u16 {
    let orig: unsafe extern "system" fn(*mut windows_sys::Win32::UI::WindowsAndMessaging::WNDCLASSEXW) -> u16 =
        unsafe { std::mem::transmute(REGISTER_CLASS_EX_W.load(Ordering::Relaxed)) };
    if !class.is_null() {
        let c = unsafe { &mut *class };
        let name = c.lpszClassName;
        if !name.is_null() && (name as usize) >> 16 != 0 {
            let mut len = 0;
            while unsafe { *name.add(len) } != 0 {
                len += 1;
            }
            if String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }) == "UnityWndClass"
                && let Some(proc_) = c.lpfnWndProc
            {
                UNITY_WNDPROC.store(proc_ as usize, Ordering::Relaxed);

                c.lpfnWndProc = Some(unity_wndproc);
            }
        }
    }
    unsafe { orig(class) }
}

const WM_SIZE: u32 = 5;
const WM_GETMINMAXINFO: u32 = 0x24;
const WM_WINDOWPOSCHANGING: u32 = 0x46;

unsafe extern "system" fn unity_wndproc(hwnd: HWND, msg: u32, wparam: usize, mut lparam: isize) -> isize {
    if msg == WM_SIZE {
        let size = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32);

        if size == screen_size() {
            lparam = ((DESKTOP.1 as isize) << 16) | DESKTOP.0 as isize;
        }
    }
    // every resize of the window (WINDOWPOS: hwnd, insert after, x, y, cx, cy, flags): the screen's
    // size at its top left corner
    if msg == WM_WINDOWPOSCHANGING && lparam != 0 {
        let pos = (lparam as *mut u8).wrapping_add(16) as *mut i32;
        let flags = unsafe { *pos.add(4) } as u32;
        let (w, h) = screen_size();
        if flags & 1 == 0 && unsafe { (*pos.add(2), *pos.add(3)) } != (w, h) {
            log!("bbg: window resize {}x{} kept {w}x{h}", unsafe { *pos.add(2) }, unsafe { *pos.add(3) });
            unsafe {
                *pos.add(2) = w;
                *pos.add(3) = h;
                if flags & 2 == 0 {
                    *pos = 0;
                    *pos.add(1) = 0;
                }
            }
        }
    }
    let orig: unsafe extern "system" fn(HWND, u32, usize, isize) -> isize = unsafe { std::mem::transmute(UNITY_WNDPROC.load(Ordering::Relaxed)) };
    let r = unsafe { orig(hwnd, msg, wparam, lparam) };
    // the maximized window (MINMAXINFO: reserved, max size, max position, min track size, max
    // track size): the real screen, not the cabinet's desktop Unity answers
    if msg == WM_GETMINMAXINFO && lparam != 0 {
        let (w, h) = screen_size();
        let info = lparam as *mut i32;
        for (i, v) in [(2, w), (3, h), (4, 0), (5, 0), (8, w), (9, h)] {
            unsafe { *info.add(i) = v };
        }
    }
    r
}

unsafe fn cabinet_rect(orig: &AtomicUsize, hwnd: HWND, rect: *mut windows_sys::Win32::Foundation::RECT) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetParent;
    let orig: GetRect = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { orig(hwnd, rect) };
    if r != 0 && !rect.is_null() && unsafe { GetParent(hwnd) }.is_null() {
        let rc = unsafe { &mut *rect };
        if (rc.right - rc.left, rc.bottom - rc.top) == screen_size() {
            rc.right = rc.left + DESKTOP.0;
            rc.bottom = rc.top + DESKTOP.1;
        }
    }
    r
}

unsafe extern "system" fn get_client_rect(hwnd: HWND, rect: *mut windows_sys::Win32::Foundation::RECT) -> i32 {
    unsafe { cabinet_rect(&GET_CLIENT_RECT, hwnd, rect) }
}

unsafe extern "system" fn get_window_rect(hwnd: HWND, rect: *mut windows_sys::Win32::Foundation::RECT) -> i32 {
    unsafe { cabinet_rect(&GET_WINDOW_RECT, hwnd, rect) }
}

/// `Screen.SetResolution(width, height, fullscreen, refresh)`: in a window.
unsafe extern "C" fn set_resolution(width: i32, height: i32, fullscreen: u8, refresh: i32) {
    log!("bbg: Screen.SetResolution({width}, {height}, {}) in a window", fullscreen != 0);
    let orig: SetResolution = unsafe { std::mem::transmute(SET_RESOLUTION.load(Ordering::Relaxed)) };
    unsafe { orig(width, height, 0, refresh) }
}

// ---- the real keyboard and mouse ----

static PEEK_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_W: AtomicUsize = AtomicUsize::new(0);
static GET_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static GET_RAW_INPUT_DATA: AtomicUsize = AtomicUsize::new(0);

type PeekMessage = unsafe extern "system" fn(*mut MSG, HWND, u32, u32, u32) -> i32;
type GetMessage = unsafe extern "system" fn(*mut MSG, HWND, u32, u32) -> i32;
type GetRawInputData = unsafe extern "system" fn(P, u32, P, *mut u32, u32) -> u32;

/// UnityPlayer's keyboard and mouse: window messages, raw input, key states.
fn filter_input() {
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    if unity == 0 {
        log!("bbg: UnityPlayer.dll not loaded: keyboard not filtered");
        return;
    }
    let hooks: [(&str, usize, Option<&AtomicUsize>); 6] = [
        ("PeekMessageA", peek_message_a as *const () as usize, Some(&PEEK_MESSAGE_A)),
        ("PeekMessageW", peek_message_w as *const () as usize, Some(&PEEK_MESSAGE_W)),
        ("GetMessageA", get_message_a as *const () as usize, Some(&GET_MESSAGE_A)),
        ("GetRawInputData", get_raw_input_data as *const () as usize, Some(&GET_RAW_INPUT_DATA)),
        ("GetAsyncKeyState", key_state as *const () as usize, None),
        ("GetKeyState", key_state as *const () as usize, None),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook_module(unity, "user32.dll", name, hook) } {
            Some(o) => {
                if let Some(slot) = orig {
                    slot.store(o, Ordering::Relaxed);
                }
            }
            None => log!("bbg: UnityPlayer does not import {name}"),
        }
    }
}

/// Keyboard and mouse messages dropped.
unsafe fn filter(msg: *mut MSG) {
    let m = unsafe { &mut *msg };
    if (WM_KEYFIRST..=WM_KEYLAST).contains(&m.message) || (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&m.message) {
        m.message = WM_NULL;
    }
}

unsafe fn peek(orig: &AtomicUsize, msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> i32 {
    let orig: PeekMessage = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { orig(msg, hwnd, min, max, remove) };
    if r != 0 && !msg.is_null() {
        unsafe { filter(msg) };
    }
    r
}

unsafe extern "system" fn peek_message_a(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek(&PEEK_MESSAGE_A, msg, hwnd, min, max, remove) }
}

unsafe extern "system" fn peek_message_w(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek(&PEEK_MESSAGE_W, msg, hwnd, min, max, remove) }
}

unsafe extern "system" fn get_message_a(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32 {
    let orig: GetMessage = unsafe { std::mem::transmute(GET_MESSAGE_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(msg, hwnd, min, max) };
    if r > 0 && !msg.is_null() {
        unsafe { filter(msg) };
    }
    r
}

const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;
/// RAWINPUTHEADER (64-bit): dwType, dwSize, hDevice, wParam.
const HEADER: usize = 24;

/// Raw input reports: mice move nowhere without buttons, keyboards release no key (virtual
/// key 0xFF).
unsafe extern "system" fn get_raw_input_data(input: P, command: u32, data: P, size: *mut u32, header_size: u32) -> u32 {
    let orig: GetRawInputData = unsafe { std::mem::transmute(GET_RAW_INPUT_DATA.load(Ordering::Relaxed)) };
    let n = unsafe { orig(input, command, data, size, header_size) };
    if command == RID_INPUT && !data.is_null() && n != u32::MAX && n as usize > HEADER {
        let report = data as *mut u8;
        unsafe {
            match (report as *const u32).read_unaligned() {
                RIM_TYPEMOUSE => std::ptr::write_bytes(report.add(HEADER), 0, n as usize - HEADER),
                RIM_TYPEKEYBOARD => {
                    // RAWKEYBOARD: MakeCode, Flags (1: break), Reserved, VKey, Message
                    let kbd = report.add(HEADER) as *mut u16;
                    kbd.write_unaligned(0);
                    kbd.add(1).write_unaligned(1);
                    kbd.add(3).write_unaligned(0xff);
                    (report.add(HEADER + 8) as *mut u32).write_unaligned(windows_sys::Win32::UI::WindowsAndMessaging::WM_KEYUP);
                }
                _ => {}
            }
        }
    }
    n
}

unsafe extern "system" fn key_state(_key: i32) -> i16 {
    0
}
