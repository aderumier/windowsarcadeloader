//! crossbeats REV. SUNRiSE (Capcom / Sega, Nu 1.1, game id SDCA; `WAL_ALLS_BOARD=cxb`): a
//! 32-bit Direct3D 9 rhythm game played on a touch screen, amLib linked in (no amdaemon):
//! the system's devices (`daemon.rs`: keychip, AMEX board, ALL.Net, the Aime reader on COM12)
//! in the game's own process, as segatools' cxbhook (public domain) does:
//!
//! - `CommIo.dll`: the REV I/O board's library (COM10: a text protocol, `SW:` switches, `CO:`
//!   coins, `AV:` amplifier volumes). Its exports are replaced by the payload's: the switches
//!   (test, service, the test menu's up / down / cancel) and coins from the launcher.
//! - `CommLamp.dll`: the LED board's library (COM11): accepted, the colors dropped.
//! - The touch screen: the game reads Windows touches (`RegisterTouchWindow`, `WM_TOUCH`,
//!   `GetTouchInputInfo`). The launcher's touch surface (`input.touch`: a touchscreen, the
//!   touchpad) gives one finger each, and the gun (`lx` / `ly`: the mouse or a lightgun) one
//!   more while `b1` is held; all are posted as `WM_TOUCH`, answered by the payload. The common
//!   crosshair option (`WAL_CROSSHAIR`, the profile's red dot) shows where the gun touches.
//! - The options (`C:\Mount\Option\NNNN`: songs, events, newest first): the executable knows
//!   the folders up to `0060`; a dump's later ones (`0061`...: its song list names songs only
//!   they hold) are seen through `0060`: a file asked there is the newest later folder's copy.

mod server;

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{gun, iat, log};
use wal_protocol::{StickState, button};
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindowVisible, PostMessageW};

pub(crate) fn init() {
    patch_exports(
        "CommIo.dll",
        &[
            ("cCommIo_Open", io_open as *const () as usize),
            ("cCommIo_Close", zero as *const () as usize),
            ("cCommIo_GetStatus", one as *const () as usize),
            ("cCommIo_GetCoin", io_get_coin as *const () as usize),
            ("cCommIo_SetCoin", ignore1 as *const () as usize),
            ("cCommIo_GetSwitch", io_get_switch as *const () as usize),
            ("cCommIo_GetTrigger", io_get_trigger as *const () as usize),
            ("cCommIo_GetRelease", io_get_release as *const () as usize),
            ("cCommIo_GetVolume", zero as *const () as usize),
            ("cCommIo_SetOutput", ignore1 as *const () as usize),
            ("cCommIo_SetAmpVolume", io_set_amp_volume as *const () as usize),
            ("cCommIo_GetAmpVolume", io_get_amp_volume as *const () as usize),
            ("cCommIo_SetAmpMute", ignore2 as *const () as usize),
            ("cCommIo_Recieve", zero as *const () as usize),
        ],
    );
    patch_exports(
        "CommLamp.dll",
        &[
            ("cCommLamp_Open", lamp_open as *const () as usize),
            ("cCommLamp_Close", zero as *const () as usize),
            ("cCommLamp_Setup", ignore1 as *const () as usize),
            ("cCommLamp_SetColor", one2 as *const () as usize),
            ("cCommLamp_Update", zero as *const () as usize),
            ("cCommLamp_UpdateDelta", zero as *const () as usize),
        ],
    );
    unsafe {
        iat::hook("user32.dll", "RegisterTouchWindow", register_touch_window as *const () as usize);
        iat::hook("user32.dll", "GetTouchInputInfo", get_touch_input_info as *const () as usize);
        iat::hook("user32.dll", "CloseTouchInputHandle", close_touch_input_handle as *const () as usize);
        iat::hook("user32.dll", "MessageBoxA", message_box_a as *const () as usize);
        if let Some(orig) = iat::hook("user32.dll", "GetSystemMetrics", get_system_metrics as *const () as usize) {
            GET_SYSTEM_METRICS.store(orig, Ordering::Relaxed);
        }
    }
    options();
    // the game server: its HTTPS requests sent in plain HTTP to the payload's server
    server::init();
    if let Some(orig) = unsafe { iat::hook("winhttp.dll", "WinHttpOpenRequest", win_http_open_request as *const () as usize) } {
        WIN_HTTP_OPEN_REQUEST.store(orig, Ordering::Relaxed);
    }
    let hooks: [(&str, usize, &AtomicUsize); 3] = [
        ("WinHttpSendRequest", win_http_send_request as *const () as usize, &WIN_HTTP_SEND_REQUEST),
        ("WinHttpWriteData", win_http_write_data as *const () as usize, &WIN_HTTP_WRITE_DATA),
        ("WinHttpReceiveResponse", win_http_receive_response as *const () as usize, &WIN_HTTP_RECEIVE_RESPONSE),
    ];
    for (name, f, slot) in hooks {
        if let Some(orig) = unsafe { iat::hook("winhttp.dll", name, f) } {
            slot.store(orig, Ordering::Relaxed);
        }
    }
    std::thread::spawn(touch_thread);
    // a red dot where the gun (or mouse) touches (the common crosshair option, `WAL_CROSSHAIR`)
    wal_payload_common::crosshair::init();
    log!("alls: cxb: REV I/O and LED libraries emulated, touch screen from the launcher");
}

/// Each named export of `dll` (the game's, loaded from its directory) replaced by a jump to the
/// payload's function: the game finds them with `GetProcAddress` once it loads the library.
fn patch_exports(dll: &str, exports: &[(&str, usize)]) {
    let name = format!("{dll}\0");
    let module = unsafe { LoadLibraryA(name.as_ptr()) };
    if module.is_null() {
        log!("alls: cxb: {dll} not found");
        return;
    }
    for &(export, f) in exports {
        let cname = format!("{export}\0");
        let Some(at) = (unsafe { GetProcAddress(module, cname.as_ptr()) }) else {
            log!("alls: cxb: {dll}!{export} not exported");
            continue;
        };
        let at = at as usize;
        let mut old = 0;
        unsafe {
            VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
            (at as *mut u8).write(0xE9);
            ((at + 1) as *mut i32).write_unaligned(f.wrapping_sub(at + 5) as i32);
            VirtualProtect(at as *const c_void, 5, old, &mut old);
        }
    }
    log!("alls: cxb: {dll} replaced");
}

extern "C" fn zero() -> i32 {
    0
}

extern "C" fn one() -> i32 {
    1
}

extern "C" fn ignore1(_a: i32) -> i32 {
    0
}

extern "C" fn ignore2(_a: i32, _b: i32) -> i32 {
    0
}

extern "C" fn one2(_a: i32, _b: i32) -> i32 {
    1
}

extern "C" fn io_open(_port: *const u8) -> i32 {
    log!("alls: cxb: REV I/O board opened");
    1
}

extern "C" fn lamp_open(_port: *const u8) -> i32 {
    log!("alls: cxb: LED board opened");
    1
}

/// The game's error boxes (its window's caption, the message in Shift-JIS) logged, never shown:
/// answered OK.
unsafe extern "system" fn message_box_a(_hwnd: HWND, text: *const u8, caption: *const u8, _kind: u32) -> i32 {
    let s = |p: *const u8| {
        if p.is_null() {
            return String::new();
        }
        let bytes = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) }.to_bytes();
        let mut wide = vec![0u16; bytes.len() + 1];
        let n = unsafe { windows_sys::Win32::Globalization::MultiByteToWideChar(932, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), wide.len() as i32) };
        String::from_utf16_lossy(&wide[..n.max(0) as usize])
    };
    log!("alls: cxb: message box {:?}: {:?}", s(caption), s(text));
    1
}

static WIN_HTTP_OPEN_REQUEST: AtomicUsize = AtomicUsize::new(0);

/// `WINHTTP_FLAG_SECURE` dropped: the game server (and ALL.Net) answered in plain HTTP.
unsafe extern "system" fn win_http_open_request(
    connect: *mut c_void,
    verb: *const u16,
    object: *const u16,
    version: *const u16,
    referrer: *const u16,
    types: *const *const u16,
    flags: u32,
) -> *mut c_void {
    type F = unsafe extern "system" fn(*mut c_void, *const u16, *const u16, *const u16, *const u16, *const *const u16, u32) -> *mut c_void;
    let orig: F = unsafe { std::mem::transmute(WIN_HTTP_OPEN_REQUEST.load(Ordering::Relaxed)) };
    unsafe { orig(connect, verb, object, version, referrer, types, flags & !0x0080_0000) }
}

// The game's asynchronous requests: in its send callback it asks for the answer
// (`WinHttpReceiveResponse`), then writes the body (`WinHttpWriteData`). Wine runs a request's
// tasks in their order: the answer awaited before the body is sent, until the socket's timeout
// (21 s). The answer is asked for once the body is written (its write queued first).

static WIN_HTTP_SEND_REQUEST: AtomicUsize = AtomicUsize::new(0);
static WIN_HTTP_WRITE_DATA: AtomicUsize = AtomicUsize::new(0);
static WIN_HTTP_RECEIVE_RESPONSE: AtomicUsize = AtomicUsize::new(0);

/// Each request with a body still to write: its bytes left.
static BODIES: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

unsafe extern "system" fn win_http_send_request(
    request: *mut c_void,
    headers: *const u16,
    headers_len: u32,
    optional: *mut c_void,
    optional_len: u32,
    total: u32,
    context: usize,
) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *const u16, u32, *mut c_void, u32, u32, usize) -> i32;
    {
        let mut bodies = BODIES.lock().unwrap();
        bodies.retain(|b| b.0 != request as usize);
        if total > optional_len {
            bodies.push((request as usize, total - optional_len));
        }
    }
    let orig: F = unsafe { std::mem::transmute(WIN_HTTP_SEND_REQUEST.load(Ordering::Relaxed)) };
    unsafe { orig(request, headers, headers_len, optional, optional_len, total, context) }
}

fn body_pending(request: usize) -> bool {
    BODIES.lock().unwrap().iter().any(|b| b.0 == request)
}

/// Asked before the body is written: asked by a thread of the payload once it is (2 s at most).
unsafe extern "system" fn win_http_receive_response(request: *mut c_void, reserved: *mut c_void) -> i32 {
    let receive = |request: usize| {
        let orig: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32 = unsafe { std::mem::transmute(WIN_HTTP_RECEIVE_RESPONSE.load(Ordering::Relaxed)) };
        unsafe { orig(request as *mut c_void, std::ptr::null_mut()) }
    };
    if !body_pending(request as usize) {
        return receive(request as usize);
    }
    let request = request as usize;
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        while body_pending(request) && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(1));
        }
        if receive(request) == 0 {
            log!("alls: cxb: WinHttpReceiveResponse failed after the body");
        }
    });
    let _ = reserved;
    1
}

unsafe extern "system" fn win_http_write_data(request: *mut c_void, buffer: *const c_void, len: u32, written: *mut u32) -> i32 {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_void, u32, *mut u32) -> i32 = unsafe { std::mem::transmute(WIN_HTTP_WRITE_DATA.load(Ordering::Relaxed)) };
    let ok = unsafe { orig(request, buffer, len, written) };
    let mut bodies = BODIES.lock().unwrap();
    if let Some(i) = bodies.iter().position(|b| b.0 == request as usize) {
        bodies[i].1 = bodies[i].1.saturating_sub(len);
        if bodies[i].1 == 0 {
            bodies.remove(i);
        }
    }
    ok
}

// The options after 0060

/// The option folders after the executable's last (`0060`), newest first.
static LATER_OPTIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);
static GET_FILE_ATTRIBUTES_W: AtomicUsize = AtomicUsize::new(0);
static GET_FILE_ATTRIBUTES_A: AtomicUsize = AtomicUsize::new(0);

const LAST_KNOWN: &str = "C:\\Mount\\Option\\0060\\";

fn options() {
    let mut later: Vec<String> = std::fs::read_dir("C:\\Mount\\Option")
        .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    later.retain(|n| n.len() == 4 && n.bytes().all(|b| b.is_ascii_digit()) && n.as_str() > "0060" && n.as_str() < "9999");
    later.sort_unstable_by(|a, b| b.cmp(a));
    if later.is_empty() {
        return;
    }
    log!("alls: cxb: options {later:?} seen through 0060");
    *LATER_OPTIONS.lock().unwrap() = later;
    let hooks: [(&str, usize, &AtomicUsize); 4] = [
        ("CreateFileW", create_file_w as *const () as usize, &CREATE_FILE_W),
        ("CreateFileA", create_file_a as *const () as usize, &CREATE_FILE_A),
        ("GetFileAttributesW", get_file_attributes_w as *const () as usize, &GET_FILE_ATTRIBUTES_W),
        ("GetFileAttributesA", get_file_attributes_a as *const () as usize, &GET_FILE_ATTRIBUTES_A),
    ];
    for (name, f, slot) in hooks {
        if let Some(orig) = unsafe { iat::hook("kernel32.dll", name, f) } {
            slot.store(orig, Ordering::Relaxed);
        }
    }
}

/// `C:\Mount\Option\0060\rest` as the newest later folder's `rest`, when one has it.
fn later_option(path: &str) -> Option<String> {
    let rest = path.get(LAST_KNOWN.len()..).filter(|_| path.get(..LAST_KNOWN.len()).is_some_and(|p| p.eq_ignore_ascii_case(LAST_KNOWN)))?;
    LATER_OPTIONS
        .lock()
        .unwrap()
        .iter()
        .map(|o| format!("C:\\Mount\\Option\\{o}\\{rest}"))
        .find(|p| std::path::Path::new(p).exists())
}

fn wide_path(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    Some(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) }))
}

fn narrow_path(p: *const u8) -> Option<String> {
    (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) }.to_string_lossy().into_owned())
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

unsafe extern "system" fn create_file_w(name: *const u16, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: *mut c_void) -> *mut c_void {
    let orig: unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void = unsafe { std::mem::transmute(CREATE_FILE_W.load(Ordering::Relaxed)) };
    match wide_path(name).and_then(|p| later_option(&p)) {
        Some(p) => unsafe { orig(wide_z(&p).as_ptr(), access, share, sa, disposition, flags, template) },
        None => unsafe { orig(name, access, share, sa, disposition, flags, template) },
    }
}

unsafe extern "system" fn create_file_a(name: *const u8, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: *mut c_void) -> *mut c_void {
    let orig: unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    match narrow_path(name).and_then(|p| later_option(&p)) {
        Some(p) => unsafe { orig(format!("{p}\0").as_ptr(), access, share, sa, disposition, flags, template) },
        None => unsafe { orig(name, access, share, sa, disposition, flags, template) },
    }
}

unsafe extern "system" fn get_file_attributes_w(name: *const u16) -> u32 {
    let orig: unsafe extern "system" fn(*const u16) -> u32 = unsafe { std::mem::transmute(GET_FILE_ATTRIBUTES_W.load(Ordering::Relaxed)) };
    match wide_path(name).and_then(|p| later_option(&p)) {
        Some(p) => unsafe { orig(wide_z(&p).as_ptr()) },
        None => unsafe { orig(name) },
    }
}

unsafe extern "system" fn get_file_attributes_a(name: *const u8) -> u32 {
    let orig: unsafe extern "system" fn(*const u8) -> u32 = unsafe { std::mem::transmute(GET_FILE_ATTRIBUTES_A.load(Ordering::Relaxed)) };
    match narrow_path(name).and_then(|p| later_option(&p)) {
        Some(p) => unsafe { orig(format!("{p}\0").as_ptr()) },
        None => unsafe { orig(name) },
    }
}

// The board's switches (segatools' cxbhook): bit 1 up, 2 cancel, 3 down, 4 test, 5 service.
const SW_UP: u32 = 1 << 1;
const SW_CANCEL: u32 = 1 << 2;
const SW_DOWN: u32 = 1 << 3;
const SW_TEST: u32 = 1 << 4;
const SW_SERVICE: u32 = 1 << 5;

/// The switches held: test, service, the test menu's up / down (player 1's) and cancel (`b2`).
fn switches() -> u32 {
    let p: StickState = wal_payload_common::input(0);
    [(button::TEST, SW_TEST), (button::SERVICE, SW_SERVICE), (button::UP, SW_UP), (button::DOWN, SW_DOWN), (button::B2, SW_CANCEL)]
        .into_iter()
        .filter(|&(b, _)| p.pressed(b))
        .fold(0, |sw, (_, bit)| sw | bit)
}

/// The switches' presses and releases since the game's last read of each.
static PRESSED: AtomicUsize = AtomicUsize::new(0);
static RELEASED: AtomicUsize = AtomicUsize::new(0);
static HELD: Mutex<u32> = Mutex::new(0);

/// The switches read: their edges since the last read added to the pending presses and releases.
fn update_switches() -> u32 {
    let now = switches();
    let mut held = HELD.lock().unwrap();
    PRESSED.fetch_or((now & !*held) as usize, Ordering::Relaxed);
    RELEASED.fetch_or((*held & !now) as usize, Ordering::Relaxed);
    *held = now;
    now
}

extern "C" fn io_get_switch() -> u32 {
    update_switches()
}

extern "C" fn io_get_trigger() -> u32 {
    update_switches();
    PRESSED.swap(0, Ordering::Relaxed) as u32
}

extern "C" fn io_get_release() -> u32 {
    update_switches();
    RELEASED.swap(0, Ordering::Relaxed) as u32
}

/// The coins inserted since the game's last read (either player's coin input).
extern "C" fn io_get_coin() -> i32 {
    static HELD: Mutex<[bool; 2]> = Mutex::new([false; 2]);
    let mut held = HELD.lock().unwrap();
    let mut coins = 0;
    for (p, was) in held.iter_mut().enumerate() {
        let now = wal_payload_common::input(p).pressed(button::COIN);
        if now && !*was {
            coins += 1;
        }
        *was = now;
    }
    coins
}

/// The amplifiers' volumes (3), as the game set them.
static AMP_VOLUME: [AtomicI32; 3] = [const { AtomicI32::new(20) }; 3];

extern "C" fn io_set_amp_volume(amp: i32, volume: i32) -> i32 {
    if let Some(v) = AMP_VOLUME.get(amp as usize) {
        v.store(volume, Ordering::Relaxed);
    }
    0
}

extern "C" fn io_get_amp_volume(amp: u32) -> i32 {
    AMP_VOLUME.get(amp as usize).map_or(0, |v| v.load(Ordering::Relaxed))
}

// The touch screen

const WM_TOUCH: u32 = 0x0240;
const TOUCHEVENTF_MOVE: u32 = 0x1;
const TOUCHEVENTF_DOWN: u32 = 0x2;
const TOUCHEVENTF_UP: u32 = 0x4;
const TOUCHEVENTF_PRIMARY: u32 = 0x10;
/// The gun's finger id (the touch surface's fingers: 1 + their index).
const GUN_FINGER: u32 = 100;
/// The `HTOUCHINPUT` handles posted: the mark and a slot of the event.
const TOUCH_MARK: usize = 0x57A3_0000;
const TOUCH_SLOTS: usize = 64;

/// A finger's event: id, flags, screen x, y.
type Touch = (u32, u32, i32, i32);
/// Each posted `WM_TOUCH`'s fingers.
static TOUCH_EVENTS: Mutex<Vec<Vec<Touch>>> = Mutex::new(Vec::new());
static TOUCH_SEQ: AtomicUsize = AtomicUsize::new(0);
/// The window the game registered for touches.
static TOUCH_WINDOW: AtomicUsize = AtomicUsize::new(0);
static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn register_touch_window(hwnd: HWND, _flags: u32) -> i32 {
    log!("alls: cxb: touch window {hwnd:?}");
    TOUCH_WINDOW.store(hwnd as usize, Ordering::Relaxed);
    1
}

unsafe extern "system" fn close_touch_input_handle(_input: *mut c_void) -> i32 {
    1
}

/// `SM_DIGITIZER` (94): an integrated, multi-touch digitizer, ready; `SM_MAXIMUMTOUCHES` (95).
unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    match index {
        94 => 0x01 | 0x40 | 0x80,
        95 => wal_protocol::MAX_TOUCHES as i32 + 1,
        _ => {
            let orig: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
            unsafe { orig(index) }
        }
    }
}

/// `TOUCHINPUT` (32-bit: 40 bytes): x, y (hundredths of screen pixels), hSource, dwID, dwFlags,
/// dwMask, dwTime, dwExtraInfo, cxContact, cyContact.
unsafe extern "system" fn get_touch_input_info(input: *mut c_void, count: u32, inputs: *mut u8, size: i32) -> i32 {
    let h = input as usize;
    let ptr = size_of::<usize>();
    let each = 32 + 2 * ptr;
    if h & !0xFFFF != TOUCH_MARK || inputs.is_null() || (size as usize) < each {
        return 0;
    }
    let events = TOUCH_EVENTS.lock().unwrap();
    let Some(fingers) = events.get(h % TOUCH_SLOTS) else { return 0 };
    let time = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() } as i32;
    for (i, &(id, flags, x, y)) in fingers.iter().take(count as usize).enumerate() {
        unsafe {
            let at = inputs.add(i * size as usize);
            std::ptr::write_bytes(at, 0, each);
            let put = |off: usize, v: i32| (at.add(off) as *mut i32).write_unaligned(v);
            put(0, x * 100);
            put(4, y * 100);
            put(8 + ptr, id as i32);
            put(12 + ptr, flags as i32);
            put(20 + ptr, time);
        }
    }
    1
}

/// The game's window: the one registered for touches, else the process's largest.
fn touch_window() -> HWND {
    match TOUCH_WINDOW.load(Ordering::Relaxed) {
        0 => gun::find_window(),
        h => h as HWND,
    }
}

/// The fingers on the game's picture: the touch surface's (id 1 + index), the gun's while `b1`
/// is held; (id, client x, y).
fn fingers(r: &RECT) -> Vec<(u32, i32, i32)> {
    let (w, h) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let mut out: Vec<(u32, i32, i32)> = wal_payload_common::touches()
        .into_iter()
        .enumerate()
        .map(|(i, (x, y))| (1 + i as u32, (x as f32 / 65536.0 * w) as i32, (y as f32 / 65536.0 * h) as i32))
        .collect();
    if wal_payload_common::input(0).pressed(button::B1)
        && let Some((u, v)) = gun::gun_in_picture()
    {
        out.push((GUN_FINGER, (u * w) as i32, (v * h) as i32));
    }
    out
}

/// The fingers posted as `WM_TOUCH` when they change: down, move, up per finger.
fn touch_thread() {
    {
        let mut events = TOUCH_EVENTS.lock().unwrap();
        events.resize(TOUCH_SLOTS, Vec::new());
    }
    let mut last: Vec<(u32, i32, i32)> = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = touch_window();
        if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 {
            continue;
        }
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if unsafe { GetClientRect(hwnd, &mut r) } == 0 || r.right <= r.left || r.bottom <= r.top {
            continue;
        }
        let now = fingers(&r);
        if now == last {
            continue;
        }
        let screen = |x: i32, y: i32| {
            let mut p = POINT { x, y };
            unsafe { ClientToScreen(hwnd, &mut p) };
            (p.x, p.y)
        };
        let mut events: Vec<Touch> = Vec::new();
        for &(id, x, y) in &now {
            let flags = match last.iter().find(|f| f.0 == id) {
                Some(&(_, lx, ly)) if (lx, ly) == (x, y) => continue,
                Some(_) => TOUCHEVENTF_MOVE,
                None => TOUCHEVENTF_DOWN,
            };
            let (sx, sy) = screen(x, y);
            events.push((id, flags, sx, sy));
        }
        for &(id, x, y) in &last {
            if !now.iter().any(|f| f.0 == id) {
                let (sx, sy) = screen(x, y);
                events.push((id, TOUCHEVENTF_UP, sx, sy));
            }
        }
        if let Some(first) = events.first_mut() {
            first.1 |= TOUCHEVENTF_PRIMARY;
        }
        last = now;
        let seq = TOUCH_SEQ.fetch_add(1, Ordering::Relaxed) % TOUCH_SLOTS;
        let n = events.len();
        TOUCH_EVENTS.lock().unwrap()[seq] = events;
        unsafe { PostMessageW(hwnd, WM_TOUCH, n, (TOUCH_MARK | seq) as isize) };
    }
}
