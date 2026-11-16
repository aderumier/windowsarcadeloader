//! Tetote X Connect (Square Enix / Taito, NESiCAxLive 2, 2023: `Vision.exe`, Unity 2018.4,
//! Mono, 64-bit, Direct3D 11): a rhythm game on the cabinet's portrait touch screen (the notes
//! touched), a depth camera (aeroTAP, the pose phases). The game's C# code calls the cabinet's
//! native plugins of `Vision_Data/Plugins` (`DllImport`), resolved to the payload's functions
//! (`mono_imports`; the payload itself is loaded at the executable's entry point, profile
//! `payload_loader`, for the portrait desktop):
//! * `VisionUSBIO.dll`: the I/O board (Test, Service, the test menu's Enter / Select, the coin
//!   counter with its decrement handshake, the LED strips) and the NESiCA reader on it (the
//!   `card` input's card, `rfid.rs`; `WAL_NESICA_CARD_AUTO=1`: on the reader for 15 seconds after
//!   each touch of the screen);
//! * `SimpleNesys.dll`: the NESYS client, Theatrhythm's (`theatrhythm_nesys.rs`): certified, the
//!   certificate's `game_server` the payload's title server (`tetote_server.rs`);
//! * `GWSafeFileDLL.dll`: the bookkeeping and operator settings (named strings), files of
//!   `tetote/system` in the game's folder; the settings' operation mode is the common free play
//!   (`WAL_FREEPLAY`), the players' language choice `WAL_TETOTE_LANGUAGE_SELECT`;
//! * `DeviceCheck.dll`: the touch panel always connected.
//!
//! Changed managed code (`Assembly-CSharp`, from a thread once Mono is up, before the first
//! scene): no depth camera is looked for (`DepthCam2.IsCameraConnected` false, the RealSense
//! `DepthCamera.Start` nothing) and none is missing (both `hasCameraProblems` false: error 301
//! otherwise; the pose phases pass without it); the camera's USB reset tool
//! (`AeroBootCheck.CheckAndReset`, an elevated `aeroBootCheckWnd.exe` up to 20 times) not run;
//! `ScreenObserver.Update` nothing (every second it set 1080x1920 full screen and rotated the
//! display).
//!
//! The touch screen: Unity reads Windows touches (`RegisterTouchWindow`, `WM_TOUCH`,
//! `GetTouchInputInfo`, resolved by the player at run time) as `Input.touches`, the only input
//! of the menus and notes. The payload's `WM_TOUCH` carry up to 10 fingers: each player's gun
//! (`lx` / `ly`: the mouse or a lightgun, players 1 and 2) while its `b1` is held, and the fingers
//! on the player's touchpad or touchscreen (the profile's `input.touch`: the whole pad is the
//! picture; `WAL_TETOTE_TOUCH=screen` a touchscreen, the picture its part); Unity's touch support is told
//! a multi-touch digitizer is ready (`GetSystemMetrics`). The real keyboard and mouse never reach
//! the game (`input_filter`). A dot where player 1's gun touches (`WAL_CROSSHAIR`).

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::json::Json;
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::mono;
use wal_payload_common::{desktop, gun, iat, input_filter, log, pointer_d3d11};
use wal_protocol::button;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, PostMessageW};

type P = *mut c_void;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the game is Tetote X Connect (`init` ran).
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The game's folder (the executable's), with its last backslash.
fn game_dir() -> String {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rfind(['\\', '/']).map_or_else(String::new, |i| path[..=i].to_string())
}

/// The game: its I/O board plugin beside the executable.
pub(crate) fn detected() -> bool {
    std::path::Path::new(&format!("{}Vision_Data\\Plugins\\VisionUSBIO.dll", game_dir())).exists()
}

pub(crate) fn init() {
    ACTIVE.store(true, Ordering::Relaxed);
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    wal_payload_common::webcam::hide_all();
    wal_payload_common::mono_imports::hook(resolve);
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    if unity == 0 {
        log!("tetote: UnityPlayer.dll not loaded: no touch screen, managed code unchanged");
        return;
    }
    let hooks: [(&str, usize, &AtomicUsize); 2] = [
        ("GetProcAddress", get_proc_address as *const () as usize, &GET_PROC_ADDRESS),
        ("GetSystemMetrics", get_system_metrics as *const () as usize, &GET_SYSTEM_METRICS),
    ];
    for (name, hook, slot) in hooks {
        let dll = if name == "GetProcAddress" { "kernel32.dll" } else { "user32.dll" };
        match unsafe { iat::hook_module(unity, dll, name, hook) } {
            Some(o) => slot.store(o, Ordering::Relaxed),
            None => log!("tetote: UnityPlayer.dll does not import {name}"),
        }
    }
    input_filter::init(unity, |_| false);
    input_filter::own_mouse(unity);
    if wal_payload_common::crosshairs(true) {
        let size = std::env::var("WAL_CROSSHAIR_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.5);
        pointer_d3d11::init(pointer_d3d11::Dot { color: [1.0, 0.125, 0.125], size: (size / 100.0).clamp(0.005, 0.2), position: gun::gun_in_picture, crosshair: false });
    }
    std::thread::spawn(touch_loop);
    log!("tetote: VisionUSBIO board, NESiCA reader, NESYS, GWSafeFile and touch screen emulated");
}

/// The game's `DllImport`s answered by the payload.
fn resolve(name: &str, module: &str) -> Option<usize> {
    use crate::theatrhythm_nesys as nesys;
    let f = match (module, name) {
        ("visionusbio.dll", "VisionUSBIOOpen") => io_open as *const () as usize,
        ("visionusbio.dll", "VisionUSBIOClose") => io_close as *const () as usize,
        ("visionusbio.dll", "VisionUSBIOUpdate") => io_update as *const () as usize,
        ("visionusbio.dll", "VisionUSBIOGetStatus") => io_status as *const () as usize,
        ("visionusbio.dll", "VisionUSBGetDeviceVersion") => io_version as *const () as usize,
        ("visionusbio.dll", "NESiCAReaderRead") => reader_read as *const () as usize,
        ("visionusbio.dll", "NESiCAReaderCancelRead") => reader_cancel as *const () as usize,
        ("visionusbio.dll", "NESiCAReaderGetStatus") => reader_status as *const () as usize,
        ("visionusbio.dll", "NESiCAReaderGetResult") => reader_result as *const () as usize,
        ("visionusbio.dll", "NESiCAReaderGetID") => reader_id as *const () as usize,
        ("visionusbio.dll", "NESiCAReaderIsError") => reader_error as *const () as usize,
        ("devicecheck.dll", "StartWatchingDevice" | "EndWatchingDevice") => nothing as *const () as usize,
        ("devicecheck.dll", "GetLastErrorCode") => zero as *const () as usize,
        ("devicecheck.dll", "IsTouchPanelEnable") => one as *const () as usize,
        ("devicecheck.dll", "CollectHIDDeviceProperties") => hid_properties as *const () as usize,
        ("gwsafefiledll.dll", "WriteStr") => safe_write as *const () as usize,
        ("gwsafefiledll.dll", "ReadStr") => safe_read as *const () as usize,
        ("gwsafefiledll.dll", "GetError") => zero as *const () as usize,
        ("simplenesys.dll", _) => match name {
            "NESYS_INIT" => nesys::NESYS_INIT as *const () as usize,
            "NESYS_RELEASE" => nesys::NESYS_RELEASE as *const () as usize,
            "NESYS_GET_LIBRARY_VER" => nesys::NESYS_GET_LIBRARY_VER as *const () as usize,
            "NESYS_GET_LANINTERFACE" => nesys::NESYS_GET_LANINTERFACE as *const () as usize,
            "NESYS_START_ONLINE_MODE" => nesys::NESYS_START_ONLINE_MODE as *const () as usize,
            "NESYS_STOP_ONLINE_MODE" => nesys::NESYS_STOP_ONLINE_MODE as *const () as usize,
            "NESYS_RESTART_ONLINE_MODE" => nesys::NESYS_RESTART_ONLINE_MODE as *const () as usize,
            "NESYS_UPDATE" => nesys::NESYS_UPDATE as *const () as usize,
            "NESYS_GET_SERVER_URL" => nesys::NESYS_GET_SERVER_URL as *const () as usize,
            "NESYS_GET_CERT_NAME" => nesys::NESYS_GET_CERT_NAME as *const () as usize,
            "NESYS_SET_DEBUG_LOGGER" => nesys::NESYS_SET_DEBUG_LOGGER as *const () as usize,
            "NESYS_GET_NETWORK_STATUS" => nesys::NESYS_GET_NETWORK_STATUS as *const () as usize,
            "NESYS_GET_NETWORK_ERROR_CODE" => nesys::NESYS_GET_NETWORK_ERROR_CODE as *const () as usize,
            "NESYS_GET_MAINTENANCE_TIME" => nesys::NESYS_GET_MAINTENANCE_TIME as *const () as usize,
            "NESYS_GET_CERTINFO" => nesys::NESYS_GET_CERTINFO as *const () as usize,
            "NESYS_GET_REQUEST_STATUS" => nesys::NESYS_GET_REQUEST_STATUS as *const () as usize,
            "NESYS_GET_REQUEST_RECIEVED_DATA" => nesys::NESYS_GET_REQUEST_RECIEVED_DATA as *const () as usize,
            "NESYS_CLOSE_REQUEST_HANDLE" => nesys::NESYS_CLOSE_REQUEST_HANDLE as *const () as usize,
            "NESYS_NOTIFY_INCOME_REQUEST" => nesys::NESYS_NOTIFY_INCOME_REQUEST as *const () as usize,
            "NESYS_GET_CARDDATA_REQUEST" => nesys::NESYS_GET_CARDDATA_REQUEST as *const () as usize,
            "NESYS_CARD_INSERT_REQUEST" => nesys::NESYS_CARD_INSERT_REQUEST as *const () as usize,
            "NESYS_CARD_REISSUE_REQUEST" => nesys::NESYS_CARD_REISSUE_REQUEST as *const () as usize,
            "NESYS_GET_PLAYLOG_REQUEST" => nesys::NESYS_GET_PLAYLOG_REQUEST as *const () as usize,
            "NESYS_PARSE_JSON" => nesys::NESYS_PARSE_JSON as *const () as usize,
            "NESYS_START_LOCALSEARCH" => nesys::NESYS_START_LOCALSEARCH as *const () as usize,
            "NESYS_GET_LOCALSEARCH_ENTRY_NUM" => nesys::NESYS_GET_LOCALSEARCH_ENTRY_NUM as *const () as usize,
            "NESYS_GET_LOCALSEARCH" => nesys::NESYS_GET_LOCALSEARCH as *const () as usize,
            _ => {
                log!("tetote: SimpleNesys!{name} not emulated");
                return None;
            }
        },
        _ => return None,
    };
    Some(f)
}

unsafe extern "C" fn nothing() {}

unsafe extern "C" fn zero() -> i32 {
    0
}

unsafe extern "C" fn one() -> i32 {
    1
}

/// No HID device to describe: an empty string (UTF-16).
unsafe extern "C" fn hid_properties(buf: *mut u16, size: u32) -> i32 {
    if !buf.is_null() && size > 0 {
        unsafe { *buf = 0 };
    }
    0
}

// ---- VisionUSBIO.dll: the I/O board ----

/// The board's switches (`IOBUFFER.InPort` bits, as `JVS_IO` decodes them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    Coin,
    Service,
    Test,
    Enter,
    Select,
}

impl Native {
    fn bit(self) -> u32 {
        match self {
            Native::Coin => 0x8000,
            Native::Service => 0x2000,
            Native::Test => 0x1000,
            Native::Enter => 0x400,
            Native::Select => 0x800,
        }
    }
}

const NATIVES: &[(&str, Native)] =
    &[("coin", Native::Coin), ("service", Native::Service), ("test", Native::Test), ("enter", Native::Enter), ("select", Native::Select)];

/// The test menu's Select / Enter on b3 / b4.
const DEFAULT_MAP: &[(&str, &str)] =
    &[("b3", "select"), ("b4", "enter"), ("test", "test"), ("service", "service"), ("coin", "coin")];

static MAP: std::sync::OnceLock<ButtonMap<Native>> = std::sync::OnceLock::new();
/// The coin counter (the board's count, less the coins the game has consumed).
static COINS: AtomicI32 = AtomicI32::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);

/// VisionUSBIO_STAT_UPDATE: the board running.
const STAT_UPDATE: i32 = 512;

/// `IOBUFFER`: InPort (u32), InAnalog (8 x u16), InEncoder (4 x u16), OutPort (u32), OutPWM (24
/// bytes), NESiCALEDControl (i32), Coin (2 x u16), CoinDec (2 x i16), IsCoinDecreasing (2 bytes),
/// padding.
const IN_PORT: usize = 0;
const OUT_PORT: usize = 28;
const COIN: usize = 60;
const COIN_DEC: usize = 64;
const COIN_DECREASING: usize = 68;

unsafe extern "C" fn io_open() -> u32 {
    log!("tetote: VisionUSBIOOpen");
    1
}

unsafe extern "C" fn io_close() -> u32 {
    1
}

/// One exchange with the board: the switches and coins in, the game's coins consumed (`CoinDec`,
/// taken off the counter at once) and outputs.
unsafe extern "C" fn io_update(buf: *mut u8) -> u32 {
    if buf.is_null() {
        return 0;
    }
    let Some(map) = MAP.get() else { return 0 };
    let mut bits = 0u32;
    let mut coin = false;
    for p in 0..2 {
        map.for_each_pressed(&wal_payload_common::input(p), |n| {
            if p == 0 || n == Native::Coin {
                bits |= n.bit();
            }
            coin |= n == Native::Coin;
        });
    }
    if coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COINS.fetch_add(1, Ordering::Relaxed);
    } else if !coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    static LAST_IN: AtomicU32 = AtomicU32::new(0);
    if LAST_IN.swap(bits, Ordering::Relaxed) != bits {
        log!("tetote: switches {bits:#06x}");
    }
    unsafe {
        let consumed = mono::read::<i16>(buf, COIN_DEC).max(0) as i32;
        if consumed > 0 {
            let _ = COINS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some((c - consumed).max(0)));
        }
        let coins = COINS.load(Ordering::Relaxed).clamp(0, 0x3FFF) as u16;
        mono::write(buf, IN_PORT, bits);
        mono::write(buf, COIN, [coins, 0]);
        mono::write(buf, COIN_DEC, [0i16, 0]);
        mono::write(buf, COIN_DECREASING, [0u8, 0]);
        let out: u32 = mono::read(buf, OUT_PORT);
        static LAST_OUT: AtomicU32 = AtomicU32::new(u32::MAX);
        if LAST_OUT.swap(out, Ordering::Relaxed) != out {
            log!("tetote: outputs {out:#06x} (0x4000 coin lockout)");
        }
    }
    1
}

unsafe extern "C" fn io_status() -> i32 {
    STAT_UPDATE
}

unsafe extern "C" fn io_version() -> i32 {
    0x100
}

// ---- the board's NESiCA reader ----

/// RFID_RESULT
const RFID_OK: u32 = 0;
const RFID_CANCEL: u32 = 3;

/// A card read in progress (until the card is in or the game cancels it).
static READING: AtomicBool = AtomicBool::new(false);
static RESULT: AtomicU32 = AtomicU32::new(RFID_OK);
/// The card of the last read on the reader.
static CARD_READ: AtomicBool = AtomicBool::new(false);
/// When a finger last touched the screen.
static LAST_TOUCH: Mutex<Option<Instant>> = Mutex::new(None);

/// The card is on the reader: the `card` input's, or with `WAL_NESICA_CARD_AUTO=1` within 15
/// seconds of a touch (the title reads the reader all along: a card always there would start a
/// credit at each attract).
fn card_present() -> bool {
    let auto = std::env::var("WAL_NESICA_CARD_AUTO").is_ok_and(|v| v.trim() == "1");
    crate::rfid::card_inserted() || auto && LAST_TOUCH.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(15))
}

/// Ends the read in progress when the card is in.
fn poll_read() {
    if READING.load(Ordering::Relaxed) && card_present() && READING.swap(false, Ordering::Relaxed) {
        log!("tetote: NESiCA {} read", crate::rfid::card_id());
        CARD_READ.store(true, Ordering::Relaxed);
        RESULT.store(RFID_OK, Ordering::Relaxed);
    }
}

unsafe extern "C" fn reader_read() -> u32 {
    if !READING.swap(true, Ordering::Relaxed) {
        CARD_READ.store(false, Ordering::Relaxed);
    }
    1
}

unsafe extern "C" fn reader_cancel() -> u32 {
    if READING.swap(false, Ordering::Relaxed) {
        RESULT.store(RFID_CANCEL, Ordering::Relaxed);
    }
    1
}

/// RFID_STATUS: 0 idle, 1 reading.
unsafe extern "C" fn reader_status() -> u32 {
    poll_read();
    READING.load(Ordering::Relaxed) as u32
}

unsafe extern "C" fn reader_result() -> u32 {
    RESULT.load(Ordering::Relaxed)
}

/// `RFID_ID`: the card id (16 ASCII digits), the NESiCA UID (7 bytes), an Amusement IC access
/// code (20 characters) and whether the card is one (no). Non-zero when there is a card.
unsafe extern "C" fn reader_id(id: *mut u8) -> u32 {
    if id.is_null() || !CARD_READ.load(Ordering::Relaxed) {
        return 0;
    }
    let mut out = [0u8; 16 + 7 + 20 + 1];
    out[..16].fill(b'0');
    for (d, c) in out.iter_mut().zip(crate::rfid::card_id().bytes().take(16)) {
        *d = c;
    }
    unsafe { std::ptr::copy_nonoverlapping(out.as_ptr(), id, out.len()) };
    1
}

unsafe extern "C" fn reader_error() -> u32 {
    0
}

// ---- GWSafeFileDLL.dll: named strings, files of tetote/system ----

/// The operator settings' string (`MachineLocalSettingManager`, JSON of setting names to
/// values).
const SETTINGS: &str = "SettingValueJson";
/// The settings' data revision (else the game resets them).
const SETTINGS_REVISION: i32 = 16;

fn safe_path(name: &str) -> Option<std::path::PathBuf> {
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    valid.then(|| std::path::PathBuf::from(format!("{}tetote\\system\\{name}.txt", game_dir())))
}

fn c_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe extern "C" fn safe_write(text: *const c_char, name: *const c_char, _max_log: i32, _commit: i32) {
    let (text, name) = (c_string(text), c_string(name));
    let Some(path) = safe_path(&name) else {
        log!("tetote: GWSafeFile name {name:?} refused");
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, &text) {
        log!("tetote: {}: {e}", path.display());
    }
}

/// The string stored under `name` (null: none), allocated for Mono to free (`CoTaskMemFree`).
unsafe extern "C" fn safe_read(name: *const c_char) -> *mut u8 {
    let name = c_string(name);
    let stored = safe_path(&name).and_then(|p| std::fs::read_to_string(p).ok());
    let text = if name == SETTINGS { Some(operator_settings(stored.as_deref())) } else { stored };
    let Some(text) = text else { return std::ptr::null_mut() };
    let p = unsafe { windows_sys::Win32::System::Com::CoTaskMemAlloc(text.len() + 1) } as *mut u8;
    if !p.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            *p.add(text.len()) = 0;
        }
    }
    p
}

/// The operator settings with the profile's: the operation mode (1: free play, `WAL_FREEPLAY`,
/// on by default), the players' language choice (`WAL_TETOTE_LANGUAGE_SELECT`, on by default).
/// Settings never saved: the game's defaults, the data revision given (else it saves its own).
fn operator_settings(stored: Option<&str>) -> String {
    let mut settings = stored.and_then(Json::parse).filter(|j| matches!(j, Json::Obj(_))).unwrap_or_else(|| Json::obj().with("DataRevision", SETTINGS_REVISION));
    let free = wal_payload_common::free_play("WAL_TETOTE_FREEPLAY").unwrap_or(true);
    let language = !std::env::var("WAL_TETOTE_LANGUAGE_SELECT").is_ok_and(|v| v.trim() == "0");
    if settings.get("DataRevision").and_then(Json::as_i64) == Some(SETTINGS_REVISION as i64) {
        settings.set("OperationMode", free as i32);
        settings.set("LanguageSelectEnable", language as i32);
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| log!("tetote: operator settings: free play {free}, language choice {language}"));
    settings.to_string()
}

// ---- the managed code ----

static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);
static ADD_INTERNAL_CALL: AtomicUsize = AtomicUsize::new(0);

/// The Unity player's `GetProcAddress`: Mono's `mono_add_internal_call` (Mono is up: the
/// managed code changed from a thread then) and the touch functions of `user32.dll`.
unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> P {
    let orig: unsafe extern "system" fn(P, *const u8) -> P = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if (name as usize) >> 16 == 0 {
        return f;
    }
    let wanted = unsafe { CStr::from_ptr(name.cast()) }.to_bytes();
    match wanted {
        b"mono_add_internal_call" if !f.is_null() => {
            ADD_INTERNAL_CALL.store(f as usize, Ordering::Relaxed);
            add_internal_call as *const () as P
        }
        b"RegisterTouchWindow" => {
            log!("tetote: Unity's touch support: the touches posted");
            register_touch_window as *const () as P
        }
        b"GetTouchInputInfo" => get_touch_input_info as *const () as P,
        b"CloseTouchInputHandle" => close_touch_input_handle as *const () as P,
        _ => f,
    }
}

unsafe extern "C" fn add_internal_call(name: *const i8, method: P) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(patch_managed);
    }
    let orig: unsafe extern "C" fn(*const i8, P) = unsafe { std::mem::transmute(ADD_INTERNAL_CALL.load(Ordering::Relaxed)) };
    unsafe { orig(name, method) }
}

unsafe extern "system" fn returns_false(_this: P) -> i32 {
    0
}

unsafe extern "system" fn does_nothing(_this: P) {}

/// The depth camera, its USB reset tool and the display rotation out of the way (once
/// `Assembly-CSharp` is loaded, before its first scene's scripts run).
fn patch_managed() {
    let start = Instant::now();
    let (m, image) = loop {
        if let Some(m) = mono::mono()
            && !m.domain().is_null()
            && m.attach()
        {
            let image = m.image("Assembly-CSharp");
            if !image.is_null() {
                break (m, image);
            }
        }
        if start.elapsed() > Duration::from_secs(120) {
            log!("tetote: Assembly-CSharp not loaded: the managed code unchanged");
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let aero = m.class(image, "Lod.ImageRecognition", "DepthCam2");
    let realsense = m.class(image, "Lod.ImageRecognition", "DepthCamera");
    // call aeroTAP::AERO_IsCameraConnected; ldc.i4.1; ceq; ret
    if let Some(il) = m.il_code(m.method(aero, "IsCameraConnected", 0))
        && il.len() == 9
        && il[0] == 0x28
        && il[5..] == [0x17, 0xFE, 0x01, 0x2A]
    {
        il.fill(0x00);
        il[..2].copy_from_slice(&[0x16, 0x2A]);
    } else {
        log!("tetote: DepthCam2.IsCameraConnected not the expected code: the camera looked for");
    }
    // ldarg.0; call get_isCameraConnected; brfalse.s; ldarg.0; call get_isCameraError; ret;
    // ldc.i4.1; ret
    if let Some(il) = m.il_code(m.method(aero, "get_hasCameraProblems", 0))
        && il.len() == 17
        && il[0] == 0x02
        && il[15..] == [0x17, 0x2A]
    {
        il.fill(0x00);
        il[..2].copy_from_slice(&[0x16, 0x2A]);
    } else {
        log!("tetote: DepthCam2.get_hasCameraProblems not the expected code: unchanged");
    }
    let replaced = [
        // its IL (ldc.i4.1; ret) is shared with other methods: its compiled code replaced
        ("DepthCamera.get_hasCameraProblems", m.replace(m.method(realsense, "get_hasCameraProblems", 0), returns_false as *const () as usize)),
        ("DepthCamera.Start", m.replace(m.method(realsense, "Start", 0), does_nothing as *const () as usize)),
        ("AeroBootCheck.CheckAndReset", m.replace(m.method(m.class(image, "", "AeroBootCheck"), "CheckAndReset", 1), returns_false as *const () as usize)),
        ("ScreenObserver.Update", m.replace(m.method(m.class(image, "", "ScreenObserver"), "Update", 0), does_nothing as *const () as usize)),
    ];
    for (name, done) in replaced {
        if !done {
            log!("tetote: {name} not replaced");
        }
    }
    log!("tetote: no depth camera (pose phases without it), no display rotation");
}

// ---- the touch screen ----

/// `TOUCHEVENTF_*`
const TOUCH_MOVE: u32 = 1;
const TOUCH_DOWN: u32 = 2;
const TOUCH_UP: u32 = 4;
const TOUCH_PRIMARY: u32 = 0x10;
const WM_TOUCH: u32 = 0x0240;
/// `SM_DIGITIZER`, `SM_MAXIMUMTOUCHES`
const SM_DIGITIZER: i32 = 94;
const SM_MAXIMUMTOUCHES: i32 = 95;
/// NID_INTEGRATED_TOUCH | NID_MULTI_INPUT | NID_READY
const DIGITIZER: i32 = 0x01 | 0x40 | 0x80;
const MAX_FINGERS: usize = 10;
/// The `HTOUCHINPUT` handles posted: the mark and a slot of the message.
const TOUCH_MARK: usize = 0x57A3_0000;
const TOUCH_SLOTS: usize = 64;

/// A finger's event: id, flags, screen x, y.
type Event = (u32, u32, i32, i32);
/// Each posted `WM_TOUCH`'s events.
static TOUCH_EVENTS: Mutex<Vec<Vec<Event>>> = Mutex::new(Vec::new());
static TOUCH_SEQ: AtomicUsize = AtomicUsize::new(0);
static REGISTERED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    match index {
        SM_DIGITIZER => DIGITIZER,
        SM_MAXIMUMTOUCHES => MAX_FINGERS as i32,
        _ => {
            let orig: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
            unsafe { orig(index) }
        }
    }
}

unsafe extern "system" fn register_touch_window(_hwnd: HWND, _flags: u32) -> i32 {
    REGISTERED.store(true, Ordering::Relaxed);
    1
}

unsafe extern "system" fn close_touch_input_handle(_input: P) -> i32 {
    1
}

/// `TOUCHINPUT` (48 bytes): x, y (hundredths of screen pixels), hSource, dwID, dwFlags, dwMask,
/// dwTime, dwExtraInfo, cxContact, cyContact.
unsafe extern "system" fn get_touch_input_info(input: P, count: u32, inputs: *mut u8, size: i32) -> i32 {
    const SIZE: usize = 48;
    let h = input as usize;
    if h & !0xFFFF != TOUCH_MARK || inputs.is_null() || (size as usize) < SIZE {
        return 0;
    }
    let events = TOUCH_EVENTS.lock().unwrap();
    let Some(events) = events.get(h % TOUCH_SLOTS) else { return 0 };
    for (i, &(id, flags, x, y)) in events.iter().take(count as usize).enumerate() {
        unsafe {
            let at = inputs.add(i * size as usize);
            std::ptr::write_bytes(at, 0, size as usize);
            let put = |off: usize, v: i32| (at.add(off) as *mut i32).write_unaligned(v);
            put(0, x * 100);
            put(4, y * 100);
            put(16, id as i32);
            put(20, flags as i32);
        }
    }
    1
}

/// The game window's client size in the units Unity takes: the fake desktop's (the window it
/// made, `WAL_DESKTOP_SIZE`; its touches' client points, `ScreenToClient`, are taken in them),
/// else the window's.
fn client_size(hwnd: HWND) -> (i32, i32) {
    if let Some(size) = desktop::client_size().or_else(desktop::size) {
        return size;
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd, &mut r) };
    (r.right, r.bottom)
}

/// A point of the player's touch surface (0..65535 across it) in the game's picture: the whole
/// surface is the picture (a touchpad, by default), or with `WAL_TETOTE_TOUCH=screen` the surface
/// is the screen and the picture its part (a touchscreen: the finger where it shows).
fn surface_in_picture((x, y): (u16, u16)) -> Option<(f32, f32)> {
    let (x, y) = (x as f32 / 65535.0, y as f32 / 65535.0);
    static SCREEN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*SCREEN.get_or_init(|| std::env::var("WAL_TETOTE_TOUCH").is_ok_and(|v| v.trim() == "screen")) {
        return Some((x.min(0.9999), y.min(0.9999)));
    }
    let (sw, sh) = gun::screen_size();
    let (sx, sy) = (x * sw, y * sh);
    let (x, y) = match desktop::size() {
        Some((w, h)) => {
            let (x, y) = desktop::from_screen(sx as f64, sy as f64);
            (x as f32 / w as f32, y as f32 / h as f32)
        }
        None => {
            let (left, top, w, h) = gun::client_area(gun::find_window())?;
            ((sx - left) / w, (sy - top) / h)
        }
    };
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

/// The touch surface's fingers with ids kept while they stay down (the launcher sends their
/// points only): each point takes the id of the nearest finger of the last report, else a new
/// one (from 10).
fn surface_fingers(points: Vec<(f32, f32)>) -> Vec<(u32, (f32, f32))> {
    static LAST: Mutex<Vec<(u32, (f32, f32))>> = Mutex::new(Vec::new());
    let mut last = LAST.lock().unwrap();
    let mut free: Vec<(u32, (f32, f32))> = last.clone();
    let mut now = Vec::new();
    for p in points {
        let nearest = free
            .iter()
            .enumerate()
            .map(|(i, (_, q))| (i, (p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|&(_, d)| d < 0.04)
            .map(|(i, _)| i);
        let id = match nearest {
            Some(i) => free.remove(i).0,
            None => (10..).find(|id| !last.iter().chain(now.iter()).any(|(l, _)| l == id)).unwrap_or(10),
        };
        now.push((id, p));
    }
    *last = now.clone();
    now
}

/// The fingers down now (id, picture fractions): the guns while their `b1` is held (ids 1, 2),
/// the touch surface's (ids 10..).
fn fingers() -> Vec<(u32, (f32, f32))> {
    let mut down = Vec::new();
    for p in 0..2 {
        if wal_payload_common::input(p).pressed(button::B1)
            && let Some(at) = gun::gun_of_player_in_picture(p)
        {
            down.push((p as u32 + 1, at));
        }
    }
    let points: Vec<(f32, f32)> = wal_payload_common::touches().into_iter().filter_map(surface_in_picture).collect();
    let room = MAX_FINGERS - down.len();
    down.extend(surface_fingers(points).into_iter().take(room));
    down
}

/// Every 8 ms: the fingers' presses, moves and releases posted as one `WM_TOUCH`.
fn touch_loop() {
    // the fingers down at the last message: id, screen point
    let mut last: Vec<(u32, (i32, i32))> = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() || !REGISTERED.load(Ordering::Relaxed) {
            continue;
        }
        let (cw, ch) = client_size(hwnd);
        let now: Vec<(u32, (i32, i32))> = fingers()
            .into_iter()
            .map(|(id, (x, y))| {
                let mut p = POINT { x: (x * cw as f32) as i32, y: (y * ch as f32) as i32 };
                unsafe { ClientToScreen(hwnd, &mut p) };
                (id, (p.x, p.y))
            })
            .collect();
        let mut events: Vec<Event> = Vec::new();
        for &(id, at) in &now {
            match last.iter().find(|(l, _)| *l == id) {
                None => events.push((id, TOUCH_DOWN, at.0, at.1)),
                Some((_, was)) if *was != at => events.push((id, TOUCH_MOVE, at.0, at.1)),
                Some(_) => {}
            }
        }
        for &(id, at) in &last {
            if !now.iter().any(|(n, _)| *n == id) {
                events.push((id, TOUCH_UP, at.0, at.1));
            }
        }
        if events.iter().any(|e| e.1 == TOUCH_DOWN) {
            *LAST_TOUCH.lock().unwrap() = Some(Instant::now());
        }
        for e in events.iter().filter(|e| e.1 != TOUCH_MOVE) {
            log!("tetote: finger {} {} at {:?}", e.0, if e.1 == TOUCH_DOWN { "down" } else { "up" }, (e.2, e.3));
        }
        last = now;
        if events.is_empty() {
            continue;
        }
        events[0].1 |= TOUCH_PRIMARY;
        let count = events.len();
        let seq = TOUCH_SEQ.fetch_add(1, Ordering::Relaxed) % TOUCH_SLOTS;
        {
            let mut slots = TOUCH_EVENTS.lock().unwrap();
            if slots.len() < TOUCH_SLOTS {
                slots.resize(TOUCH_SLOTS, Vec::new());
            }
            slots[seq] = events;
        }
        unsafe { PostMessageW(hwnd, WM_TOUCH, count, (TOUCH_MARK | seq) as isize) };
    }
}
