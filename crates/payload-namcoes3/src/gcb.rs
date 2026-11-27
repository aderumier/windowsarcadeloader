//! Mobile Suit Gundam U.C. Card Builder (Bandai Namco, 2016, FromSoftware engine, Direct3D 11,
//! 64-bit), `GAC_MasterArcade.exe` (ALL.Net SDBR, MUCHA GCB1 34.04, build 13109). The game
//! imports nothing of ours: the payload is loaded at the executable's entry point
//! (`wal-loader64`, profile `payload_loader`).
//!
//! - dongle: `nbamUsbFinder.dll` (serial [`SERIAL`]).
//! - I/O board: the USIO on USB through the game's `winusio.dll` (`CWinUsio`, WinUSB: `usio.rs`).
//! - network: the AMCUS auth daemon (`IAuth`) in-process (`amcus.rs`).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::StickState;

use crate::{module, usio};

pub(crate) const GAME: &str = "GAC_MasterArcade.exe";

const SERIAL: &str = "284013030001";
const NET_ID: &str = "ABLN3030001";
const GAME_ID: &str = "SDBR";
const GAME_CD: &str = "GCB1";

pub(crate) fn is_gcb(exe: &str) -> bool {
    exe.eq_ignore_ascii_case(GAME)
}

// --- the USB dongle ---

unsafe extern "system" fn usb_finder_ok() -> u64 {
    0
}

/// `nbamUsbFinderGetSerialNumber(index, out)`: the dongle's serial.
unsafe extern "system" fn usb_finder_serial(_index: i32, out: *mut u8) -> u64 {
    if !out.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(SERIAL.as_ptr(), out, SERIAL.len());
            *out.add(SERIAL.len()) = 0;
        }
    }
    0
}

// --- the two displays ---

static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);
static ENUM_DISPLAY_DEVICES_W: AtomicUsize = AtomicUsize::new(0);

/// `GetSystemMetrics(SM_CMONITORS)`: the cabinet's two monitors (the boot waits for two).
unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    const SM_CMONITORS: i32 = 80;
    if index == SM_CMONITORS {
        return 2;
    }
    let real: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
    unsafe { real(index) }
}

/// `EnumDisplayDevicesW(NULL, i, ..)`: two adapters' outputs attached to the desktop,
/// `\\.\DISPLAY1` (primary) and `\\.\DISPLAY2`.
unsafe extern "system" fn enum_display_devices_w(device: *const u16, index: u32, out: *mut u8, flags: u32) -> i32 {
    if device.is_null() {
        if index >= 2 || out.is_null() {
            return 0;
        }
        // DISPLAY_DEVICEW: cb, DeviceName[32], DeviceString[128], StateFlags, DeviceID[128], DeviceKey[128]
        let cb = unsafe { *(out as *const u32) } as usize;
        unsafe { std::ptr::write_bytes(out.add(4), 0, cb.saturating_sub(4)) };
        let put = |at: usize, text: &str| {
            for (i, c) in text.encode_utf16().enumerate() {
                unsafe { *(out.add(at + i * 2) as *mut u16) = c };
            }
        };
        put(4, &format!("\\\\.\\DISPLAY{}", index + 1));
        // not "AMD Radeon(TM)": the game takes that GPU for the shop's terminal (+0x511890: one
        // 1280x800 screen, the shop server started); another one is the player's seat (satellite)
        put(4 + 64, "WAL display");
        // DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE (the first)
        unsafe { *(out.add(4 + 64 + 256) as *mut u32) = if index == 0 { 0x5 } else { 0x1 } };
        return 1;
    }
    type F = unsafe extern "system" fn(*const u16, u32, *mut u8, u32) -> i32;
    let real: F = unsafe { std::mem::transmute(ENUM_DISPLAY_DEVICES_W.load(Ordering::Relaxed)) };
    unsafe { real(device, index, out, flags) }
}

/// `ChangeDisplaySettingsExW`: both monitors in 1920x1080, done (nothing changed).
unsafe extern "system" fn change_display_settings_ex_w(_name: *const u16, _mode: *const u8, _hwnd: usize, _flags: u32, _param: usize) -> i32 {
    0
}

fn hook_displays(base: usize) {
    let hooks: [(&str, usize, Option<&AtomicUsize>); 3] = [
        ("GetSystemMetrics", get_system_metrics as *const () as usize, Some(&GET_SYSTEM_METRICS)),
        ("EnumDisplayDevicesW", enum_display_devices_w as *const () as usize, Some(&ENUM_DISPLAY_DEVICES_W)),
        ("ChangeDisplaySettingsExW", change_display_settings_ex_w as *const () as usize, None),
    ];
    for (name, f, slot) in hooks {
        match unsafe { iat::hook_module(base, "user32.dll", name, f) } {
            Some(o) => {
                if let Some(slot) = slot {
                    slot.store(o, Ordering::Relaxed);
                }
            }
            None => log!("gcb: {name} not hooked"),
        }
    }
}

// --- the shop server ---

static CREATE_PROCESS_W: AtomicUsize = AtomicUsize::new(0);

/// The cabinet starts the shop server's watchdog (`WatchdogForServer_vc2012.exe`, which starts
/// `TenpoServer.exe`) with `-noServerWindow false` in a console of its own (+0x454e80): started
/// with its window hidden and no console (the console took the screen over the game).
unsafe extern "system" fn create_process_w(app: *const u16, cmd: *mut u16, pa: usize, ta: usize, inherit: i32, flags: u32, env: usize, dir: *const u16, si: usize, pi: usize) -> i32 {
    type F = unsafe extern "system" fn(*const u16, *mut u16, usize, usize, i32, u32, usize, *const u16, usize, usize) -> i32;
    let real: F = unsafe { std::mem::transmute(CREATE_PROCESS_W.load(Ordering::Relaxed)) };
    let wide = |p: *const u16| -> String {
        if p.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    };
    let (app_s, cmd_s) = (wide(app), wide(cmd));
    log!("gcb: starts {app_s} {cmd_s}");
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if cmd_s.contains("-noServerWindow") {
        let mut line: Vec<u16> = cmd_s.replace("-noServerWindow false", "-noServerWindow true").encode_utf16().chain(Some(0)).collect();
        return unsafe { real(app, line.as_mut_ptr(), pa, ta, inherit, (flags & !CREATE_NEW_CONSOLE) | CREATE_NO_WINDOW, env, dir, si, pi) };
    }
    unsafe { real(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }
}

static SENDTO: AtomicUsize = AtomicUsize::new(0);

/// `sendto`: the seat looks for the shop server with a `client_hello` broadcast on the shop's
/// network (GACClientUtil, +0x9fa940), answered `server_hello` by `TenpoServer.exe`: sent to this
/// PC's server (127.0.0.1, the same port).
unsafe extern "system" fn sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    type F = unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;
    let real: F = unsafe { std::mem::transmute(SENDTO.load(Ordering::Relaxed)) };
    if !buf.is_null() && len == 12 && !to.is_null() && tolen >= 16 && unsafe { std::slice::from_raw_parts(buf, 12) } == b"client_hello" {
        let mut addr = [0u8; 16];
        unsafe { std::ptr::copy_nonoverlapping(to, addr.as_mut_ptr(), 16) };
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| log!("gcb: shop server search to {}.{}.{}.{}:{}, sent to 127.0.0.1", addr[4], addr[5], addr[6], addr[7], u16::from_be_bytes([addr[2], addr[3]])));
        addr[4..8].copy_from_slice(&[127, 0, 0, 1]);
        return unsafe { real(s, buf, len, flags, addr.as_ptr(), 16) };
    }
    unsafe { real(s, buf, len, flags, to, tolen) }
}

// --- the USIO board ---

#[derive(Clone, Copy)]
enum Native {
    Test,
    Service,
    Coin,
    Switch(u32),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Switch(0x8000)),
    ("up", Native::Switch(0x2000)),
    ("down", Native::Switch(0x1000)),
    ("left", Native::Switch(0x0800)),
    ("right", Native::Switch(0x0400)),
    ("push1", Native::Switch(0x0200)),
    ("push2", Native::Switch(0x0100)),
    ("push3", Native::Switch(0x0040)),
    ("push4", Native::Switch(0x0020)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "push1"),
    ("b2", "push2"),
    ("b3", "push3"),
    ("b4", "push4"),
];

const SW_SERVICE: u32 = 0x4000;
const SW_TEST: u32 = 0x80;

struct State {
    map: ButtonMap<Native>,
    coins: u16,
    coin_held: bool,
    test_on: bool,
    test_held: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn usio_poll() -> usio::Inputs {
    let mut guard = STATE.lock().unwrap();
    let s = guard.get_or_insert_with(|| State { map: ButtonMap::new(NATIVES, DEFAULT_MAP), coins: 0, coin_held: false, test_on: false, test_held: false });
    let stick: StickState = wal_payload_common::input(0);
    let mut sw = 0u32;
    let (mut coin, mut test) = (false, false);
    s.map.for_each_pressed(&stick, |n| match n {
        Native::Test => test = true,
        Native::Coin => coin = true,
        Native::Service => sw |= SW_SERVICE,
        Native::Switch(bit) => sw |= bit,
    });
    if test && !s.test_held {
        s.test_on = !s.test_on;
        log!("gcb: test switch {}", if s.test_on { "on" } else { "off" });
    }
    s.test_held = test;
    if s.test_on {
        sw |= SW_TEST;
    }
    if coin && !s.coin_held {
        s.coins = s.coins.wrapping_add(1) & 0x3FFF;
    }
    s.coin_held = coin;
    usio::Inputs { switches: sw, coins: [s.coins, 0], analog: [0; 8] }
}

// --- the cabinet authentication ---

static CO_CREATE_INSTANCE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut c_void, context: u32, iid: *const u8, out: *mut *mut c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = crate::amcus::daemon(&crate::amcus::OBJECT_NEW);
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        log!("gcb: in-process AMCUS auth (IAuth)");
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

pub(crate) fn attach(base: usize) {
    crate::GAME_BASE.store(base, Ordering::Relaxed);
    log!("gcb: build {:#x}", crate::exe_timestamp(base));
    for (name, f) in [
        ("nbamUsbFinderInitialize", usb_finder_ok as *const () as usize),
        ("nbamUsbFinderGetSerialNumber", usb_finder_serial as *const () as usize),
    ] {
        if unsafe { iat::hook_module(base, "nbamUsbFinder.dll", name, f) }.is_none() {
            log!("gcb: {name} not hooked");
        }
    }
    match unsafe { iat::hook_module(base, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => CO_CREATE_INSTANCE.store(o, Ordering::Relaxed),
        None => log!("gcb: CoCreateInstance not hooked"),
    }
    hook_displays(base);
    match unsafe { iat::hook_module(base, "kernel32.dll", "CreateProcessW", create_process_w as *const () as usize) } {
        Some(o) => CREATE_PROCESS_W.store(o, Ordering::Relaxed),
        None => log!("gcb: CreateProcessW not hooked"),
    }
    match unsafe { iat::hook_ordinal(base, "ws2_32.dll", 20, sendto as *const () as usize) } {
        Some(o) => SENDTO.store(o, Ordering::Relaxed),
        None => log!("gcb: sendto not hooked"),
    }
    crate::gcb_screens::init(base);
    if std::env::var("WAL_NAMCOES3_GCB_TRACE").is_ok_and(|v| v == "1") {
        crate::gcb_trace::init(base);
    }
    crate::amcus::set_identity(crate::amcus::Identity { game_id: GAME_ID, game_cd: GAME_CD, game_ver: (34, 4), serial: SERIAL, scheme: "", path: "", net_id: NET_ID });
    let winusio = module("winusio.dll");
    if winusio == 0 {
        log!("gcb: winusio.dll not loaded, no I/O board");
    } else {
        usio::init(winusio, usio_poll);
        usio::set_memory(0x12, &126u16.to_le_bytes());
        usio::set_memory(0x1800, b"NBGI.REAL_USIO\0");
    }
    crate::game_pads_hidden(base);
    wal_payload_common::input_filter::init(base, |_| false);
}
