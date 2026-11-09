//! MAME's raw input (Galaxian 3's patched `mame.exe`): the players' guns and the operator keys
//! from the virtual sticks, the real keyboard and mouse hidden.
//!
//! MAME's `rawinput` providers (keyboard and lightgun, the profile's arguments) list the raw
//! input devices once (`GetRawInputDeviceList`, each device's name from
//! `GetRawInputDeviceInfoW`), register their window for `WM_INPUT` (`RegisterRawInputDevices`)
//! and read each report there (`GetRawInputData`), finding its device by handle. The
//! executable's imports of these are hooked:
//!
//! * the device list holds the payload's devices only: a keyboard and one absolute mouse per
//!   player, which MAME's lightgun provider takes as guns `GUNCODE_<n>` (it adds the list from
//!   its end: the guns are listed last to first, player 1's is gun 1). The real keyboard and
//!   mouse are not listed: their reports name no device MAME knows, it drops them.
//! * a thread posts the payload's reports to MAME's window (`WM_INPUT`, a marked handle whose
//!   report `GetRawInputData` answers): each gun's position (absolute, 0..65535 over MAME's
//!   window: its picture) and its buttons' presses and releases, and the keys of the `key_*`
//!   natives (scan codes, MAME's rawinput key codes).
//!
//! Natives (each player's gun): `trigger`, `trigger2` (gun buttons 1, 2), `start`, `coin`
//! (buttons 3, 4: the profile's MAME controller file `ctrlr/wal.cfg` maps them to the game's
//! starts and coins); the keyboard's `key_<name>` (every player's alike).
//!
//! `WAL_MAME_STRETCH=1`: the picture stretched to the screen (MAME's `keepaspect` off), written
//! at boot to the system's own ini (`ini\<system>.ini` of the run directory, read over
//! `mame.ini`; MAME's command line takes no value for its switches). The profile's code
//! patches (`WAL_PATCHES`: Galaxian 3's startup popups skipped) are applied first.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::gun::{client_area, on_screen, screen_size};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use wal_protocol::MAX_PLAYERS;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

type P = *mut c_void;

/// The payload's device handles: the keyboard, then the guns (`DEVICE + 1 + player`).
const DEVICE: usize = 0x57A2_0100;
const KEYBOARD: usize = DEVICE;
/// The payload's report handles (`REPORT` and a slot).
const REPORT: usize = 0x57A3_0000;
const SLOTS: usize = 256;

const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;
const RID_INPUT: u32 = 0x1000_0003;
const RID_HEADER: u32 = 0x1000_0005;
const RIDI_DEVICENAME: u32 = 0x2000_0007;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
/// RAWINPUTHEADER: type, size, device, wParam.
const HEADER: usize = 8 + 2 * size_of::<usize>();
/// RAWMOUSE (the largest body MAME reads of ours).
const BODY: usize = 24;
const MOUSE_MOVE_ABSOLUTE: u16 = 1;
const RI_KEY_BREAK: u16 = 1;
const RI_KEY_E0: u16 = 2;
/// Reports posted at least this often (a gun held still, a window resized).
const REFRESH: Duration = Duration::from_millis(100);

static GET_RAW_INPUT_DATA: AtomicUsize = AtomicUsize::new(0);
static GET_RAW_INPUT_DEVICE_INFO_W: AtomicUsize = AtomicUsize::new(0);
static REGISTER_RAW_INPUT_DEVICES: AtomicUsize = AtomicUsize::new(0);
/// MAME's window for `WM_INPUT` (its registration's target).
static TARGET: AtomicUsize = AtomicUsize::new(0);
/// The posted reports: their device, then their body, by slot.
static SEQ: AtomicUsize = AtomicUsize::new(0);
static SLOT_DEVICE: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static SLOT_BODY: [[AtomicU64; BODY / 8]; SLOTS] = [const { [const { AtomicU64::new(0) }; BODY / 8] }; SLOTS];

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A gun button (0-based: trigger 0, trigger2 1, start 2, coin 3).
    Gun(u8),
    /// A keyboard key: its scan code, extended (E0).
    Key(u16, bool),
}

/// Key names of the `key_<name>` natives (the digits and letters are their own names), scan
/// codes and extended.
const KEY_NAMES: &[(&str, u16, bool)] = &[
    ("escape", 0x01, false),
    ("minus", 0x0C, false),
    ("equals", 0x0D, false),
    ("backspace", 0x0E, false),
    ("tab", 0x0F, false),
    ("enter", 0x1C, false),
    ("lctrl", 0x1D, false),
    ("lshift", 0x2A, false),
    ("rshift", 0x36, false),
    ("lalt", 0x38, false),
    ("space", 0x39, false),
    ("up", 0x48, true),
    ("left", 0x4B, true),
    ("right", 0x4D, true),
    ("down", 0x50, true),
    ("insert", 0x52, true),
    ("delete", 0x53, true),
    ("home", 0x47, true),
    ("end", 0x4F, true),
    ("pageup", 0x49, true),
    ("pagedown", 0x51, true),
];

/// Scan codes of the digits and letters (set 1).
const DIGITS: &[u8] = b"1234567890";
const LETTERS: &[(&[u8], u16)] = &[(b"qwertyuiop", 0x10), (b"asdfghjkl", 0x1E), (b"zxcvbnm", 0x2C)];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "trigger2"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "key_f2"),
    // MAME's menus and popups (its keyboard: the arrows, Enter selects; Tab opens its menu)
    ("up", "key_up"),
    ("down", "key_down"),
    ("left", "key_left"),
    ("right", "key_right"),
    ("b3", "key_enter"),
    ("service", "key_tab"),
];

fn natives() -> &'static [(&'static str, Native)] {
    static NATIVES: OnceLock<Vec<(&'static str, Native)>> = OnceLock::new();
    NATIVES.get_or_init(|| {
        let mut n = vec![("trigger", Native::Gun(0)), ("trigger2", Native::Gun(1)), ("start", Native::Gun(2)), ("coin", Native::Gun(3))];
        let mut keys: Vec<(String, u16, bool)> = KEY_NAMES.iter().map(|&(name, scan, e0)| (name.to_string(), scan, e0)).collect();
        keys.extend(DIGITS.iter().enumerate().map(|(i, &c)| ((c as char).to_string(), 0x02 + i as u16, false)));
        for &(row, first) in LETTERS {
            keys.extend(row.iter().enumerate().map(|(i, &c)| ((c as char).to_string(), first + i as u16, false)));
        }
        // F1-F10 0x3B..0x44, F11 0x57, F12 0x58
        keys.extend((1..=12u16).map(|f| (format!("f{f}"), if f <= 10 { 0x3A + f } else { 0x4C + f }, false)));
        for (name, scan, e0) in keys {
            n.push((Box::leak(format!("key_{name}").into_boxed_str()), Native::Key(scan, e0)));
        }
        n
    })
}

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(natives(), DEFAULTS))
}

fn gun_device(player: usize) -> usize {
    DEVICE + 1 + player
}

/// The payload's device of handle `h`: its name (MAME's device id).
fn device_name(h: usize) -> Option<String> {
    if h == KEYBOARD {
        Some("\\\\?\\HID#WAL_KEYBOARD#1#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}".into())
    } else if (gun_device(0)..gun_device(MAX_PLAYERS)).contains(&h) {
        Some(format!("\\\\?\\HID#WAL_GUN_{}#1#{{378de44c-56ef-11d1-bc8c-00a0c91405dd}}", h - gun_device(0) + 1))
    } else {
        None
    }
}

unsafe extern "system" fn get_raw_input_device_list(list: *mut [usize; 2], count: *mut u32, size: u32) -> u32 {
    let n = 1 + MAX_PLAYERS as u32;
    if count.is_null() || size as usize != 2 * size_of::<usize>() {
        return u32::MAX;
    }
    if list.is_null() {
        unsafe { *count = n };
        return 0;
    }
    if unsafe { *count } < n {
        unsafe {
            *count = n;
            windows_sys::Win32::Foundation::SetLastError(ERROR_INSUFFICIENT_BUFFER);
        }
        return u32::MAX;
    }
    // RAWINPUTDEVICELIST: device, type. MAME adds them from the last: player 1's gun last.
    unsafe {
        *list = [KEYBOARD, RIM_TYPEKEYBOARD as usize];
        for i in 0..MAX_PLAYERS {
            *list.add(1 + i) = [gun_device(MAX_PLAYERS - 1 - i), RIM_TYPEMOUSE as usize];
        }
    }
    log!("mame: raw input devices listed: the payload's keyboard and {MAX_PLAYERS} guns");
    n
}

unsafe extern "system" fn get_raw_input_device_info_w(device: P, command: u32, data: *mut u16, size: *mut u32) -> u32 {
    let Some(name) = device_name(device as usize) else {
        let f: unsafe extern "system" fn(P, u32, *mut u16, *mut u32) -> u32 =
            unsafe { std::mem::transmute(GET_RAW_INPUT_DEVICE_INFO_W.load(Ordering::Relaxed)) };
        return unsafe { f(device, command, data, size) };
    };
    if command != RIDI_DEVICENAME || size.is_null() {
        return u32::MAX;
    }
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    if data.is_null() {
        unsafe { *size = wide.len() as u32 };
        return 0;
    }
    if (unsafe { *size } as usize) < wide.len() {
        unsafe {
            *size = wide.len() as u32;
            windows_sys::Win32::Foundation::SetLastError(ERROR_INSUFFICIENT_BUFFER);
        }
        return u32::MAX;
    }
    unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), data, wide.len()) };
    wide.len() as u32
}

unsafe extern "system" fn get_raw_input_data(raw: P, command: u32, data: *mut u8, size: *mut u32, header_size: u32) -> u32 {
    let h = raw as usize;
    if h & !0xFFFF != REPORT {
        let f: unsafe extern "system" fn(P, u32, *mut u8, *mut u32, u32) -> u32 =
            unsafe { std::mem::transmute(GET_RAW_INPUT_DATA.load(Ordering::Relaxed)) };
        return unsafe { f(raw, command, data, size, header_size) };
    }
    let slot = h % SLOTS;
    let device = SLOT_DEVICE[slot].load(Ordering::Acquire);
    let kind = if device == KEYBOARD { RIM_TYPEKEYBOARD } else { RIM_TYPEMOUSE };
    let full = match command {
        RID_HEADER => HEADER,
        RID_INPUT => HEADER + BODY,
        _ => return u32::MAX,
    };
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
    let mut report = [0u8; HEADER + BODY];
    report[..4].copy_from_slice(&kind.to_le_bytes());
    report[4..8].copy_from_slice(&((HEADER + BODY) as u32).to_le_bytes());
    report[8..8 + size_of::<usize>()].copy_from_slice(&device.to_le_bytes());
    for (i, word) in SLOT_BODY[slot].iter().enumerate() {
        report[HEADER + 8 * i..HEADER + 8 * i + 8].copy_from_slice(&word.load(Ordering::Relaxed).to_le_bytes());
    }
    unsafe { std::ptr::copy_nonoverlapping(report.as_ptr(), data, full) };
    full as u32
}

/// RAWINPUTDEVICE: usage page, usage, flags, target window.
#[repr(C)]
struct RawInputDevice {
    page: u16,
    usage: u16,
    flags: u32,
    target: P,
}

unsafe extern "system" fn register_raw_input_devices(devices: *const RawInputDevice, count: u32, size: u32) -> i32 {
    if !devices.is_null() && count > 0 {
        let target = unsafe { (*devices).target };
        if !target.is_null() && TARGET.swap(target as usize, Ordering::Relaxed) != target as usize {
            log!("mame: raw input window {target:?}");
        }
    }
    let f: unsafe extern "system" fn(*const RawInputDevice, u32, u32) -> i32 =
        unsafe { std::mem::transmute(REGISTER_RAW_INPUT_DEVICES.load(Ordering::Relaxed)) };
    unsafe { f(devices, count, size) }
}

/// Posts a report of `device`, its body `body`, to MAME's window.
fn post(hwnd: P, device: usize, body: [u8; BODY]) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_INPUT};
    let slot = SEQ.fetch_add(1, Ordering::Relaxed) % SLOTS;
    for (i, word) in SLOT_BODY[slot].iter().enumerate() {
        word.store(u64::from_le_bytes(body[8 * i..8 * i + 8].try_into().unwrap()), Ordering::Relaxed);
    }
    SLOT_DEVICE[slot].store(device, Ordering::Release);
    // RIM_INPUTSINK: MAME reads its reports in the background too
    unsafe { PostMessageW(hwnd, WM_INPUT, 1, (REPORT | slot) as isize) };
}

/// A gun report: absolute position `x`, `y` (0..65535), button transitions `flags`.
fn gun_report(x: i32, y: i32, flags: u16) -> [u8; BODY] {
    let mut b = [0u8; BODY];
    b[..2].copy_from_slice(&MOUSE_MOVE_ABSOLUTE.to_le_bytes());
    b[4..6].copy_from_slice(&flags.to_le_bytes());
    b[12..16].copy_from_slice(&x.to_le_bytes());
    b[16..20].copy_from_slice(&y.to_le_bytes());
    b
}

fn key_report(scan: u16, e0: bool, down: bool) -> [u8; BODY] {
    let mut b = [0u8; BODY];
    let flags = if down { 0 } else { RI_KEY_BREAK } | if e0 { RI_KEY_E0 } else { 0 };
    b[..2].copy_from_slice(&scan.to_le_bytes());
    b[2..4].copy_from_slice(&flags.to_le_bytes());
    b
}

/// Player `player`'s gun on MAME's window `area` (0..65535 from its top left).
fn gun_position(player: usize, area: Option<(f32, f32, f32, f32)>) -> (i32, i32) {
    let screen = screen_size();
    let (sx, sy) = on_screen(&input(player), screen);
    let (left, top, w, h) = area.unwrap_or((0.0, 0.0, screen.0, screen.1));
    let fx = ((sx - left) / w).clamp(0.0, 1.0);
    let fy = ((sy - top) / h).clamp(0.0, 1.0);
    ((fx * 65535.0) as i32, (fy * 65535.0) as i32)
}

/// Writes `ini\<system>.ini` (the system: MAME's first argument) with the aspect option.
fn write_ini() {
    let Some(system) = std::env::args().nth(1).filter(|a| !a.starts_with('-')) else { return };
    let stretch = std::env::var("WAL_MAME_STRETCH").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "on"));
    let text = format!("# written by the payload at each start (WAL_MAME_STRETCH)\r\nkeepaspect                {}\r\n", if stretch { 0 } else { 1 });
    let path = std::path::Path::new("ini").join(format!("{system}.ini"));
    match std::fs::create_dir_all("ini").and_then(|_| std::fs::write(&path, text)) {
        Ok(()) => log!("mame: {} (keepaspect {})", path.display(), if stretch { 0 } else { 1 }),
        Err(e) => log!("mame: {}: {e}", path.display()),
    }
}

/// Installs the hooks in the executable's imports and starts the reports.
pub fn init() {
    wal_payload_common::patches::apply();
    write_ini();
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let hooks: [(&str, usize, Option<&AtomicUsize>); 4] = [
        ("GetRawInputDeviceList", get_raw_input_device_list as *const () as usize, None),
        ("GetRawInputDeviceInfoW", get_raw_input_device_info_w as *const () as usize, Some(&GET_RAW_INPUT_DEVICE_INFO_W)),
        ("GetRawInputData", get_raw_input_data as *const () as usize, Some(&GET_RAW_INPUT_DATA)),
        ("RegisterRawInputDevices", register_raw_input_devices as *const () as usize, Some(&REGISTER_RAW_INPUT_DEVICES)),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook_module(exe, "user32.dll", name, hook) } {
            Some(o) => {
                if let Some(slot) = orig {
                    slot.store(o, Ordering::Relaxed);
                }
            }
            None => log!("mame: the executable does not import user32!{name}"),
        }
    }
    std::thread::spawn(run);
}

fn run() {
    let mut buttons = [0u8; MAX_PLAYERS];
    let mut positions = [(-1, -1); MAX_PLAYERS];
    let mut keys: Vec<(u16, bool)> = Vec::new();
    let mut last = Instant::now();
    let mut last_area = None;
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let hwnd = TARGET.load(Ordering::Relaxed) as P;
        if hwnd.is_null() {
            continue;
        }
        let area = client_area(hwnd);
        if area != last_area {
            log!("mame: window area {area:?}");
            last_area = area;
        }
        let refresh = last.elapsed() >= REFRESH;
        let mut held = Vec::new();
        for (player, (pressed, position)) in buttons.iter_mut().zip(positions.iter_mut()).enumerate() {
            let mut now = 0u8;
            map().for_each_pressed(&input(player), |n| match n {
                Native::Gun(b) => now |= 1 << b,
                Native::Key(scan, e0) => {
                    if !held.contains(&(scan, e0)) {
                        held.push((scan, e0));
                    }
                }
            });
            // RI_MOUSE_BUTTON_<n>_DOWN 1 << 2n, _UP 2 << 2n
            let mut flags = 0u16;
            for b in 0..5 {
                match (*pressed & 1 << b != 0, now & 1 << b != 0) {
                    (false, true) => flags |= 1 << (2 * b),
                    (true, false) => flags |= 2 << (2 * b),
                    _ => {}
                }
            }
            let at = gun_position(player, area);
            if flags != 0 || at != *position || refresh {
                post(hwnd, gun_device(player), gun_report(at.0, at.1, flags));
                *pressed = now;
                *position = at;
            }
        }
        for &(scan, e0) in held.iter().filter(|k| !keys.contains(k)) {
            log!("mame: key {scan:#04x} down");
            post(hwnd, KEYBOARD, key_report(scan, e0, true));
        }
        for &(scan, e0) in keys.iter().filter(|k| !held.contains(k)) {
            log!("mame: key {scan:#04x} up");
            post(hwnd, KEYBOARD, key_report(scan, e0, false));
        }
        keys = held;
        if refresh {
            last = Instant::now();
        }
    }
}
