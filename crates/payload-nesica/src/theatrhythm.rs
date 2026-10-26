//! Theatrhythm Final Fantasy All-Star Carnival (Square Enix, NESiCAxLive 2, Unity 5.3 Mono,
//! 64-bit): the game's C# code calls three native plugins of `game_Data/Plugins` (DllImport),
//! all replaced by this DLL (the profile installs it under the three names):
//! * `ismACIO.dll`: the I/O board (on top of `iDmacDrv64.dll`): the two levers (4 directions)
//!   with their lamp switches, the test panel's Select / Enter / Test / Service, the coin
//!   counter, lamp outputs;
//! * `NESiCAReader.dll`: the card reader (Taito XIO protocol on a COM port originally): the
//!   `card` virtual input's card (`rfid.rs`, `WAL_NESICA_CARD_ID`);
//! * `SimpleNesys.dll`: the NESYS client (HTTPS REST to cert2.nesys.jp with the cabinet's
//!   client certificate): online, the card server the payload's own (`theatrhythm_nesys.rs`,
//!   `theatrhythm_server.rs`, `theatrhythm_player.rs`: NESiCA play, the players' records kept),
//!   no matching with other cabinets.
//!
//! Mono loads each plugin by its full path, three modules of the same DLL with three states:
//! the first one loaded hooks the game's and Mono's `LoadLibraryW`/`LoadLibraryExW`, the other
//! two names load this module.
//!
//! The touch panel's tool (`NanoTS_PTool_win8.exe`, run by the game: setup, calibration, then
//! its connection state every few seconds) answers "Not connected" without the panel's USB
//! controller: error 15001. Mono's `CreateProcessW` runs `cmd.exe /c echo Connected` instead.
//!
//! The cabinet's touch screen is read as Unity's mouse (`Input.GetMouseButton`,
//! `Input.mousePosition`: the game's touch panel on Windows): the pointer is player 1's `accel`
//! (x) and `brake` (y) axes (a gun's or a mouse's position, `0..32767` across the screen), the
//! touch the `b7` virtual button. Unity's window messages and `GetCursorPos` (game.exe, the
//! player) are hooked as Chase Chase Jokers': real keyboard and mouse messages dropped, the
//! payload's own posted. The game hides the cursor (a cabinet has a touch screen); so that a
//! mouse player sees where the touch lands, `WAL_THEATRHYTHM_POINTER` shows the pointer: `dot`
//! (default) a red dot drawn over the picture (`pointer_d3d11.rs`, `WAL_THEATRHYTHM_POINTER_SIZE`
//! its diameter in percent of the screen height), `arrow` the Windows cursor kept shown and
//! moved to the pointer, `none` nothing. Unity's pads (raw input HID, XInput:
//! `Joystick1Button0` = the left lamp switch) and raw input are hidden.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::time::Instant;

use wal_payload_common::{drive, iat, log, mapping::ButtonMap, pointer_d3d11};
use wal_protocol::{Axis, button};
use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleHandleExW, GetModuleHandleW};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, HCURSOR, IDC_ARROW, LoadCursorW, SetCursor, SetCursorPos, GetClientRect, GetWindowThreadProcessId, IsWindowVisible, MSG, PostMessageW, WM_KEYFIRST, WM_KEYLAST,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEFIRST, WM_MOUSELAST, WM_MOUSEMOVE, WM_NULL,
};

type P = *mut c_void;

/// The plugin names this DLL is installed as.
const PLUGINS: [&str; 3] = ["ismACIO.dll", "NESiCAReader.dll", "SimpleNesys.dll"];

pub(crate) fn detected(module: HMODULE) -> bool {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    let name = path.rsplit(['\\', '/']).next().unwrap_or_default();
    PLUGINS.iter().any(|p| p.eq_ignore_ascii_case(name))
}

pub(crate) fn init() {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    // the game's D:\ data, opened by Mono's file functions
    if !drive::redirect_module("mono.dll") {
        log!("theatrhythm: mono.dll not loaded: D: not redirected");
    }
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let mono = unsafe { GetModuleHandleW(windows_sys::w!("mono.dll")) } as usize;
    for base in [exe, mono].into_iter().filter(|b| *b != 0) {
        if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", "LoadLibraryW", load_library_w as *const () as usize) } {
            LOAD_LIBRARY_W.store(o, Ordering::Relaxed);
        }
        if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", "LoadLibraryExW", load_library_ex_w as *const () as usize) } {
            LOAD_LIBRARY_EX_W.store(o, Ordering::Relaxed);
        }
    }
    if mono != 0 {
        match unsafe { iat::hook_module(mono, "kernel32.dll", "CreateProcessW", create_process_w as *const () as usize) } {
            Some(o) => CREATE_PROCESS_W.store(o, Ordering::Relaxed),
            None => log!("theatrhythm: mono.dll does not import CreateProcessW: touch panel tool not replaced"),
        }
    }
    init_input(exe);
    log!("theatrhythm: ismACIO board, NESiCA reader and NESYS emulated");
}

// ---- one module for the three plugins ----

static LOAD_LIBRARY_W: AtomicUsize = AtomicUsize::new(0);
static LOAD_LIBRARY_EX_W: AtomicUsize = AtomicUsize::new(0);

/// This module when `name` is one of the plugins (a new reference, as a load would take).
fn this_module(name: *const u16) -> Option<HMODULE> {
    let path = wide_string(name);
    let file = path.rsplit(['\\', '/']).next().unwrap_or_default();
    if !PLUGINS.iter().any(|p| p.eq_ignore_ascii_case(file) || p.trim_end_matches(".dll").eq_ignore_ascii_case(file)) {
        return None;
    }
    let mut module: HMODULE = std::ptr::null_mut();
    unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, this_module as *const () as *const u16, &mut module) };
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| log!("theatrhythm: {path} -> this module (and the other plugins)"));
    Some(module)
}

unsafe extern "system" fn load_library_w(name: *const u16) -> HMODULE {
    if let Some(m) = this_module(name) {
        return m;
    }
    let orig: unsafe extern "system" fn(*const u16) -> HMODULE = unsafe { std::mem::transmute(LOAD_LIBRARY_W.load(Ordering::Relaxed)) };
    unsafe { orig(name) }
}

unsafe extern "system" fn load_library_ex_w(name: *const u16, file: P, flags: u32) -> HMODULE {
    if let Some(m) = this_module(name) {
        return m;
    }
    let orig: unsafe extern "system" fn(*const u16, P, u32) -> HMODULE = unsafe { std::mem::transmute(LOAD_LIBRARY_EX_W.load(Ordering::Relaxed)) };
    unsafe { orig(name, file, flags) }
}

// ---- the touch panel's tool ----

static CREATE_PROCESS_W: AtomicUsize = AtomicUsize::new(0);

type CreateProcessW = unsafe extern "system" fn(*const u16, *mut u16, P, P, i32, u32, P, *const u16, P, P) -> i32;

fn wide_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

unsafe extern "system" fn create_process_w(
    app: *const u16, cmd: *mut u16, pa: P, ta: P, inherit: i32, flags: u32, env: P, cwd: *const u16, si: P, pi: P,
) -> i32 {
    let orig: CreateProcessW = unsafe { std::mem::transmute(CREATE_PROCESS_W.load(Ordering::Relaxed)) };
    let line = format!("{} {}", wide_string(app), wide_string(cmd));
    if !line.to_ascii_lowercase().contains("nanots_ptool") {
        return unsafe { orig(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi) };
    }
    let answer = if line.contains("connection:state") { "cmd.exe /c echo Connected" } else { "cmd.exe /c exit 0" };
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| log!("theatrhythm: touch panel tool ({}) -> {answer}", line.trim()));
    let mut fake: Vec<u16> = answer.encode_utf16().chain(Some(0)).collect();
    unsafe { orig(std::ptr::null(), fake.as_mut_ptr(), pa, ta, inherit, flags, env, cwd, si, pi) }
}

// ---- ismACIO.dll: the I/O board ----

/// The board's switches (`ismACIO_GetInputD` bits, as the game decodes them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    LUp,
    LDown,
    LLeft,
    LRight,
    LLamp,
    RUp,
    RDown,
    RLeft,
    RRight,
    RLamp,
    Select,
    Enter,
    Test,
    Service,
    Coin,
}

impl Native {
    fn bit(self) -> u32 {
        match self {
            Native::RUp => 0x1,
            Native::RDown => 0x2,
            Native::RLeft => 0x4,
            Native::RRight => 0x8,
            Native::LLamp => 0x20,
            Native::Enter => 0x80,
            Native::LUp => 0x100,
            Native::LDown => 0x200,
            Native::LLeft => 0x400,
            Native::LRight => 0x800,
            Native::Select => 0x1000,
            Native::Test => 0x2000,
            Native::Service => 0x4000,
            Native::Coin => 0x8000,
            Native::RLamp => 0x10000,
        }
    }
}

const NATIVES: &[(&str, Native)] = &[
    ("lup", Native::LUp),
    ("ldown", Native::LDown),
    ("lleft", Native::LLeft),
    ("lright", Native::LRight),
    ("llamp", Native::LLamp),
    ("rup", Native::RUp),
    ("rdown", Native::RDown),
    ("rleft", Native::RLeft),
    ("rright", Native::RRight),
    ("rlamp", Native::RLamp),
    ("select", Native::Select),
    ("enter", Native::Enter),
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
];

/// The left lever on the directions, its switch on b1; the right lever's switch on b2 (its
/// directions are the right stick's); the test panel's Select / Enter (test menu) on b3 / b4.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "lup"),
    ("down", "ldown"),
    ("left", "lleft"),
    ("right", "lright"),
    ("b1", "llamp"),
    ("b2", "rlamp"),
    ("b3", "select"),
    ("b4", "enter"),
    ("start", "enter"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

static MAP: std::sync::OnceLock<ButtonMap<Native>> = std::sync::OnceLock::new();
static COINS: AtomicI32 = AtomicI32::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);
static OUTPUT_D: AtomicU32 = AtomicU32::new(0);

/// A lever direction from a stick axis past half way.
fn lever(stick: &wal_protocol::StickState, x: Axis, y: Axis) -> u32 {
    const HALF: i16 = 16384;
    let mut bits = 0;
    if stick.axis(y) < -HALF {
        bits |= Native::RUp.bit();
    }
    if stick.axis(y) > HALF {
        bits |= Native::RDown.bit();
    }
    if stick.axis(x) < -HALF {
        bits |= Native::RLeft.bit();
    }
    if stick.axis(x) > HALF {
        bits |= Native::RRight.bit();
    }
    bits
}

#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_Init() -> i32 {
    log!("theatrhythm: ismACIO_Init");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_Delete() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_Update() -> u32 {
    // coins: one per press
    let pressed = MAP.get().is_some_and(|m| {
        let mut coin = false;
        for p in 0..wal_protocol::MAX_PLAYERS {
            m.for_each_pressed(&wal_payload_common::input(p), |n| coin |= n == Native::Coin);
        }
        coin
    });
    if pressed && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COINS.fetch_add(1, Ordering::Relaxed);
    } else if !pressed {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    0
}

/// 2: OK.
#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_GetStatus() -> i32 {
    2
}

#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_GetInputD() -> u32 {
    let Some(map) = MAP.get() else { return 0 };
    let stick = wal_payload_common::input(0);
    let mut bits = lever(&stick, Axis::RightX, Axis::RightY);
    map.for_each_pressed(&stick, |n| bits |= n.bit());
    static LAST: AtomicU32 = AtomicU32::new(0);
    if LAST.swap(bits, Ordering::Relaxed) != bits {
        log!("theatrhythm: switches {bits:#07x}");
    }
    bits
}

/// The analog inputs (8 x u16): none used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ismACIO_GetInputA(value: *mut u16) {
    if !value.is_null() {
        unsafe { std::ptr::write_bytes(value, 0, 8) };
    }
}

/// Lamps: the NESiCA reader's RGB (0x1-0x4), the lamp switches (0x8, 0x10), the coin lockout
/// (0x4000).
#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_SetOutputD(value: u32) {
    if OUTPUT_D.swap(value, Ordering::Relaxed) != value {
        log!("theatrhythm: lamps {value:#06x}");
    }
}

/// The RGB LED strips (24 bytes): not shown.
#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_SetOutputA(_value: *const u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_GetCoin(_port: i32) -> u32 {
    COINS.load(Ordering::Relaxed).max(0) as u32
}

/// `num` coins consumed (negative: added, the test menu's credit button).
#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_DecreaseCoin(_port: i32, num: i32) {
    let _ = COINS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some((c - num).max(0)));
}

/// 0: OK (no jam).
#[unsafe(no_mangle)]
pub extern "C" fn ismACIO_GetCoinStatus(_port: i32) -> i32 {
    0
}

// ---- NESiCAReader.dll: the card reader ----

/// XioStatIdle: the reader is connected.
const XIO_IDLE: i32 = 1;
/// NESICARESDER_RESULT
const READER_OK: u32 = 0;
const READER_BUSY: u32 = 3;
/// RFID_RESULT
const RFID_OK: u32 = 0;
const RFID_TIMEOUT: u32 = 3;
const RFID_CANCEL: u32 = 4;

struct Read {
    since: Instant,
    timeout_ms: u32,
}

/// The read in progress.
static READ: Mutex<Option<Read>> = Mutex::new(None);
static RESULT: AtomicU32 = AtomicU32::new(RFID_OK);

/// Ends the read in progress when the card is in or its time is out.
fn poll_read() {
    let mut read = READ.lock().unwrap();
    let Some(r) = read.as_ref() else { return };
    if crate::rfid::card_inserted() {
        log!("theatrhythm: NESiCA {} read", crate::rfid::card_id());
        RESULT.store(RFID_OK, Ordering::Relaxed);
        *read = None;
    } else if r.since.elapsed().as_millis() >= r.timeout_ms as u128 {
        RESULT.store(RFID_TIMEOUT, Ordering::Relaxed);
        *read = None;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderOpen() -> u32 {
    log!("theatrhythm: NESiCAReaderOpen");
    READER_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderClose() -> u32 {
    READER_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderUpdate() -> u32 {
    poll_read();
    READER_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderGetXioStatus() -> i32 {
    XIO_IDLE
}

/// Starts a card read, `timeout` in milliseconds.
#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderRead(timeout: u32) -> u32 {
    let mut read = READ.lock().unwrap();
    if read.is_some() {
        return READER_BUSY;
    }
    log!("theatrhythm: card read ({timeout} ms)");
    *read = Some(Read { since: Instant::now(), timeout_ms: timeout });
    drop(read);
    crate::rfid::card_requested();
    READER_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderCancelRead() -> u32 {
    if READ.lock().unwrap().take().is_some() {
        RESULT.store(RFID_CANCEL, Ordering::Relaxed);
    }
    READER_OK
}

/// RFID_STATUS: 0 idle, 1 reading.
#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderGetStatus() -> u32 {
    poll_read();
    READ.lock().unwrap().is_some() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn NESiCAReaderGetResult() -> u32 {
    RESULT.load(Ordering::Relaxed)
}

/// The card id (16 ASCII characters); non-zero when there is one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn NESiCAReaderGetID(id: *mut u8) -> u32 {
    if id.is_null() || !crate::rfid::card_inserted() {
        return 0;
    }
    let mut digits = [b'0'; 16];
    for (d, c) in digits.iter_mut().zip(crate::rfid::card_id().bytes()) {
        *d = c;
    }
    unsafe { std::ptr::copy_nonoverlapping(digits.as_ptr(), id, 16) };
    1
}

// ---- the touch screen (Unity's mouse) and the real devices hidden ----

/// `wParam` bit of the payload's mouse messages (no MK_ flag uses it).
const TAG: usize = 0x8000;
const MK_LBUTTON: usize = 1;
const VK_LBUTTON: i32 = 1;

static GET_CURSOR_POS: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static PEEK_MESSAGE_W: AtomicUsize = AtomicUsize::new(0);
static GET_MESSAGE_A: AtomicUsize = AtomicUsize::new(0);
static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static WINDOW: AtomicUsize = AtomicUsize::new(0);

type PeekMessage = unsafe extern "system" fn(*mut MSG, HWND, u32, u32, u32) -> i32;

fn init_input(exe: usize) {
    let shown = std::env::var("WAL_THEATRHYTHM_POINTER").unwrap_or_default();
    if shown.trim() == "arrow" {
        SHOW_CURSOR.store(true, Ordering::Relaxed);
        for (name, hook) in [("ShowCursor", show_cursor as *const () as usize), ("SetCursor", set_cursor as *const () as usize)] {
            if unsafe { iat::hook_module(exe, "user32.dll", name, hook) }.is_none() {
                log!("theatrhythm: game.exe does not import {name}");
            }
        }
    } else if shown.trim() != "none" {
        let size = std::env::var("WAL_THEATRHYTHM_POINTER_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0);
        pointer_d3d11::init(pointer_d3d11::Dot { color: [1.0, 0.125, 0.125], size: (size / 100.0).clamp(0.005, 0.2), position: dot_position, crosshair: false });
    }
    let hooks: [(&str, &str, usize, Option<&AtomicUsize>); 9] = [
        ("user32.dll", "GetCursorPos", get_cursor_pos as *const () as usize, Some(&GET_CURSOR_POS)),
        ("user32.dll", "PeekMessageA", peek_message_a as *const () as usize, Some(&PEEK_MESSAGE_A)),
        ("user32.dll", "PeekMessageW", peek_message_w as *const () as usize, Some(&PEEK_MESSAGE_W)),
        ("user32.dll", "GetMessageA", get_message_a as *const () as usize, Some(&GET_MESSAGE_A)),
        ("user32.dll", "GetAsyncKeyState", key_state as *const () as usize, None),
        ("user32.dll", "GetKeyState", key_state as *const () as usize, None),
        ("user32.dll", "RegisterRawInputDevices", register_raw_input_devices as *const () as usize, None),
        ("user32.dll", "GetRawInputDeviceList", get_raw_input_device_list as *const () as usize, None),
        ("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize, Some(&GET_PROC_ADDRESS)),
    ];
    for (dll, name, hook, orig) in hooks {
        match unsafe { iat::hook_module(exe, dll, name, hook) } {
            Some(o) => {
                if let Some(slot) = orig {
                    slot.store(o, Ordering::Relaxed);
                }
            }
            None => log!("theatrhythm: game.exe does not import {name}"),
        }
    }
    std::thread::spawn(touch_loop);
}

fn touching() -> bool {
    wal_payload_common::input(0).pressed(button::B7)
}

/// The Unity window of this process, once it exists.
fn window() -> HWND {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != unsafe { GetCurrentProcessId() } || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut class = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } as usize;
        if String::from_utf16_lossy(&class[..n]) == "UnityWndClass" {
            unsafe { *(found as *mut HWND) = hwnd };
            return 0;
        }
        1
    }
    let mut w = WINDOW.load(Ordering::Relaxed) as HWND;
    if w.is_null() {
        unsafe { EnumWindows(Some(each), &mut w as *mut HWND as LPARAM) };
        WINDOW.store(w as usize, Ordering::Relaxed);
    }
    w
}

/// The pointer in the window's client area.
fn pointer(window: HWND) -> (i32, i32) {
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(window, &mut rect) };
    let stick = wal_payload_common::input(0);
    let at = |axis: Axis, size: i32| ((stick.axis(axis).max(0) as i64 * (size - 1).max(0) as i64) / 32767) as i32;
    (at(Axis::Accel, rect.right), at(Axis::Brake, rect.bottom))
}

/// The dot's place (fractions of the picture), once the pointer has moved.
fn dot_position() -> Option<(f32, f32)> {
    static MOVED: AtomicBool = AtomicBool::new(false);
    let stick = wal_payload_common::input(0);
    let (x, y) = (stick.axis(Axis::Accel).max(0), stick.axis(Axis::Brake).max(0));
    if !MOVED.load(Ordering::Relaxed) {
        if x == 0 && y == 0 {
            return None;
        }
        MOVED.store(true, Ordering::Relaxed);
    }
    Some((x as f32 / 32767.0, y as f32 / 32767.0))
}

/// Every 10 ms: the pointer's moves and the touch's presses, posted to the game's window.
fn touch_loop() {
    let mut last = (-1, -1);
    let mut down = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let w = window();
        if w.is_null() {
            continue;
        }
        let pos = pointer(w);
        let touch = touching();
        let lparam = ((pos.1 as u32 & 0xffff) << 16 | (pos.0 as u32 & 0xffff)) as LPARAM;
        let buttons = if touch { MK_LBUTTON } else { 0 };
        if pos != last {
            last = pos;
            unsafe { PostMessageW(w, WM_MOUSEMOVE, TAG | buttons, lparam) };
            if SHOW_CURSOR.load(Ordering::Relaxed) {
                let mut p = POINT { x: pos.0, y: pos.1 };
                unsafe {
                    ClientToScreen(w, &mut p);
                    SetCursorPos(p.x, p.y);
                }
            }
        }
        if touch != down {
            down = touch;
            log!("theatrhythm: touch {} at {pos:?}", if touch { "down" } else { "up" });
            let msg = if touch { WM_LBUTTONDOWN } else { WM_LBUTTONUP };
            if unsafe { PostMessageW(w, msg, TAG | buttons, lparam) } == 0 {
                WINDOW.store(0, Ordering::Relaxed);
            }
        }
    }
}

unsafe extern "system" fn get_cursor_pos(point: *mut POINT) -> i32 {
    let w = window();
    if w.is_null() || point.is_null() {
        let orig: unsafe extern "system" fn(*mut POINT) -> i32 = unsafe { std::mem::transmute(GET_CURSOR_POS.load(Ordering::Relaxed)) };
        return unsafe { orig(point) };
    }
    let (x, y) = pointer(w);
    let mut p = POINT { x, y };
    unsafe {
        ClientToScreen(w, &mut p);
        *point = p;
    }
    1
}

/// The cursor shown at the pointer (`WAL_THEATRHYTHM_POINTER=arrow`).
static SHOW_CURSOR: AtomicBool = AtomicBool::new(false);

/// The game's hide requests ignored: the cursor stays shown.
unsafe extern "system" fn show_cursor(_show: i32) -> i32 {
    0
}

/// No cursor (the game's hidden one) becomes the arrow.
unsafe extern "system" fn set_cursor(cursor: HCURSOR) -> HCURSOR {
    let cursor = if cursor.is_null() { unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) } } else { cursor };
    unsafe { SetCursor(cursor) }
}

/// The real keyboard and mouse messages dropped, the payload's untagged.
unsafe fn filter(msg: *mut MSG) {
    let m = unsafe { &mut *msg };
    if (WM_KEYFIRST..=WM_KEYLAST).contains(&m.message) {
        m.message = WM_NULL;
    } else if (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&m.message) {
        if m.wParam & TAG != 0 {
            m.wParam &= !TAG;
        } else {
            m.message = WM_NULL;
        }
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
    let orig: unsafe extern "system" fn(*mut MSG, HWND, u32, u32) -> i32 = unsafe { std::mem::transmute(GET_MESSAGE_A.load(Ordering::Relaxed)) };
    let r = unsafe { orig(msg, hwnd, min, max) };
    if r > 0 && !msg.is_null() {
        unsafe { filter(msg) };
    }
    r
}

/// Only the left button, while touching.
unsafe extern "system" fn key_state(key: i32) -> i16 {
    if key == VK_LBUTTON && touching() { i16::MIN } else { 0 }
}

/// No raw input (keyboard, mice, HID pads): registered as asked, nothing registered.
unsafe extern "system" fn register_raw_input_devices(_devices: P, _count: u32, _size: u32) -> i32 {
    1
}

/// No raw input device.
unsafe extern "system" fn get_raw_input_device_list(_list: P, count: *mut u32, _size: u32) -> u32 {
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    0
}

/// ERROR_DEVICE_NOT_CONNECTED: no XInput pad.
unsafe extern "system" fn xinput_none(_user: u32, _state: P) -> u32 {
    1167
}

/// Unity's XInput functions (loaded at run time) see no pad.
unsafe extern "system" fn get_proc_address(module: HMODULE, name: *const u8) -> P {
    if name as usize > 0xffff {
        let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
        if n == b"XInputGetState" || n == b"XInputGetCapabilities" {
            return xinput_none as *const () as P;
        }
    }
    let orig: unsafe extern "system" fn(HMODULE, *const u8) -> P = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    unsafe { orig(module, name) }
}
