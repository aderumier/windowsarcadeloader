//! Chase Chase Jokers' trackball: a raw input mouse the game looks up by its device name
//! (`VID_1241&PID_1111`), whose `WM_INPUT` reports it reads in its main window's procedure
//! (the window it subclasses, `ChaseProject`).
//!
//! Wine's user32 calls win32u for raw input (`GetRawInputDeviceList`, `GetRawInputDeviceInfoW`
//! are jumps through user32's imports of `NtUserGetRawInputDeviceList` /
//! `NtUserGetRawInputDeviceInfo`; `GetRawInputData` calls `NtUserGetRawInputData`): user32's
//! imports are hooked, so the game's `DllImport`s get ours. The device list has one more mouse,
//! the trackball; once the game found it, a thread posts `WM_INPUT` with the trackball's own
//! report handle to the game's window while the virtual stick turns it, and `GetRawInputData`
//! answers that handle with the motion gathered since the last report.
//!
//! The real mice and keyboards' reports (Unity's) carry no motion, button or key: the game's
//! input comes from the virtual sticks only.
//!
//! The trackball is player 1's right stick (`rx`, `ry`): full tilt moves it
//! `WAL_KONAMI_TRACKBALL_SPEED` counts per 10 ms (default 12).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::{HWND, LPARAM, SetLastError};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_INPUT, WM_KEYUP};

type P = *mut c_void;

/// The trackball's device handle and its report handle (odd values: never real ones).
const DEVICE: usize = 0x7ba1_1001;
const REPORT: usize = 0x7ba1_1003;
const DEVICE_NAME: &str = "\\\\?\\HID#VID_1241&PID_1111#7&ba11&0&0000#{378de44c-56ef-11d1-bc8c-00a0c91405dd}";

const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;
const RID_HEADER: u32 = 0x1000_0005;
const RID_INPUT: u32 = 0x1000_0003;
const RIDI_DEVICENAME: u32 = 0x2000_0007;
const RIDI_DEVICEINFO: u32 = 0x2000_000b;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
/// RAWINPUTHEADER (type, size, device, wParam) and RAWINPUT (header + RAWMOUSE).
const HEADER: usize = 24;
const RAWINPUT: usize = HEADER + 24;
/// RAWINPUTDEVICELIST: device handle, type (padded).
const DEVICE_LIST_ENTRY: usize = 16;

static GET_DEVICE_LIST: AtomicUsize = AtomicUsize::new(0);
static GET_DEVICE_INFO: AtomicUsize = AtomicUsize::new(0);
static GET_RAW_INPUT_DATA: AtomicUsize = AtomicUsize::new(0);

/// The game opened the trackball: reports may be posted.
static FOUND: AtomicBool = AtomicBool::new(false);
/// A report is posted and not read yet.
static PENDING: AtomicBool = AtomicBool::new(false);
/// Motion not reported yet (counts).
static MOTION: [AtomicI32; 2] = [AtomicI32::new(0), AtomicI32::new(0)];
/// Counts per tick at full tilt (f32 bits).
static SPEED: AtomicU32 = AtomicU32::new(0);

type GetDeviceList = unsafe extern "system" fn(P, *mut u32, u32) -> u32;
type GetDeviceInfo = unsafe extern "system" fn(P, u32, P, *mut u32) -> u32;
type GetRawInputData = unsafe extern "system" fn(P, u32, P, *mut u32, u32) -> u32;

pub fn init() {
    let speed = std::env::var("WAL_KONAMI_TRACKBALL_SPEED").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(12.0);
    SPEED.store(speed.to_bits(), Ordering::Relaxed);
    let user32 = unsafe { GetModuleHandleW(windows_sys::w!("user32.dll")) } as usize;
    let hooks: [(&str, usize, &AtomicUsize); 3] = [
        ("NtUserGetRawInputDeviceList", get_device_list as *const () as usize, &GET_DEVICE_LIST),
        ("NtUserGetRawInputDeviceInfo", get_device_info as *const () as usize, &GET_DEVICE_INFO),
        ("NtUserGetRawInputData", get_raw_input_data as *const () as usize, &GET_RAW_INPUT_DATA),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook_module(user32, "win32u.dll", name, hook) } {
            Some(o) => orig.store(o, Ordering::Relaxed),
            None => log!("ccj: user32 does not import win32u!{name}: no trackball"),
        }
    }
    std::thread::spawn(report_loop);
}

/// Every 10 ms: the right stick's motion, posted to the game's window as a report.
fn report_loop() {
    let mut rest = [0f32; 2];
    let mut window: HWND = std::ptr::null_mut();
    let mut waited = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if !FOUND.load(Ordering::Relaxed) {
            continue;
        }
        let stick = wal_payload_common::input(0);
        let speed = f32::from_bits(SPEED.load(Ordering::Relaxed));
        for (i, axis) in [Axis::RightX, Axis::RightY].into_iter().enumerate() {
            let v = stick.axis(axis) as f32 / 32767.0 * speed + rest[i];
            let counts = v.trunc();
            rest[i] = v - counts;
            if counts != 0.0 {
                MOTION[i].fetch_add(counts as i32, Ordering::Relaxed);
            }
        }
        if PENDING.load(Ordering::Relaxed) {
            // a report nobody read (the window not subclassed yet): post again later
            waited += 1;
            if waited < 10 {
                continue;
            }
        }
        waited = 0;
        if MOTION.iter().all(|m| m.load(Ordering::Relaxed) == 0) {
            continue;
        }
        if window.is_null() {
            window = game_window();
            if window.is_null() {
                continue;
            }
            log!("ccj: trackball reports to window {window:?}");
        }
        PENDING.store(true, Ordering::Relaxed);
        if unsafe { PostMessageW(window, WM_INPUT, 0, REPORT as LPARAM) } == 0 {
            PENDING.store(false, Ordering::Relaxed);
            window = std::ptr::null_mut();
        }
    }
}

/// The game's visible window titled `ChaseProject` (its product name).
pub(crate) fn game_window() -> HWND {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != unsafe { GetCurrentProcessId() } || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut title = [0u16; 128];
        let n = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) } as usize;
        if String::from_utf16_lossy(&title[..n]).contains("ChaseProject") {
            unsafe { *(found as *mut HWND) = hwnd };
            return 0;
        }
        1
    }
    let mut found: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
    found
}

/// The real devices, then the trackball.
unsafe extern "system" fn get_device_list(list: P, count: *mut u32, size: u32) -> u32 {
    let orig: GetDeviceList = unsafe { std::mem::transmute(GET_DEVICE_LIST.load(Ordering::Relaxed)) };
    if count.is_null() || size as usize != DEVICE_LIST_ENTRY {
        return unsafe { orig(list, count, size) };
    }
    let mut real = 0;
    if unsafe { orig(std::ptr::null_mut(), &mut real, size) } == u32::MAX {
        return u32::MAX;
    }
    if list.is_null() {
        unsafe { *count = real + 1 };
        return 0;
    }
    if unsafe { *count } < real + 1 {
        unsafe {
            *count = real + 1;
            SetLastError(ERROR_INSUFFICIENT_BUFFER);
        }
        return u32::MAX;
    }
    let mut room = unsafe { *count };
    let n = unsafe { orig(list, &mut room, size) };
    if n == u32::MAX {
        return n;
    }
    unsafe {
        let entry = (list as *mut u8).add(n as usize * DEVICE_LIST_ENTRY);
        std::ptr::write_bytes(entry, 0, DEVICE_LIST_ENTRY);
        (entry as *mut usize).write_unaligned(DEVICE);
        (entry.add(8) as *mut u32).write_unaligned(RIM_TYPEMOUSE);
    }
    n + 1
}

/// The trackball's name (the game looks for its USB id) and mouse info.
unsafe extern "system" fn get_device_info(device: P, command: u32, data: P, size: *mut u32) -> u32 {
    if device as usize != DEVICE {
        let orig: GetDeviceInfo = unsafe { std::mem::transmute(GET_DEVICE_INFO.load(Ordering::Relaxed)) };
        return unsafe { orig(device, command, data, size) };
    }
    if size.is_null() {
        return u32::MAX;
    }
    match command {
        RIDI_DEVICENAME => {
            let name: Vec<u16> = DEVICE_NAME.encode_utf16().chain(Some(0)).collect();
            let len = name.len() as u32;
            if data.is_null() {
                unsafe { *size = len };
                return 0;
            }
            if unsafe { *size } < len {
                unsafe {
                    *size = len;
                    SetLastError(ERROR_INSUFFICIENT_BUFFER);
                }
                return u32::MAX;
            }
            unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), data as *mut u16, name.len()) };
            if !FOUND.swap(true, Ordering::Relaxed) {
                log!("ccj: the game found the trackball");
            }
            len
        }
        RIDI_DEVICEINFO => {
            // RID_DEVICE_INFO: cbSize, dwType, RID_DEVICE_INFO_MOUSE (id, buttons, sample rate,
            // horizontal wheel)
            const LEN: u32 = 32;
            if data.is_null() || unsafe { *size } < LEN {
                unsafe { *size = LEN };
                return if data.is_null() { 0 } else { u32::MAX };
            }
            let info: [u32; 8] = [LEN, RIM_TYPEMOUSE, 0x100, 3, 0, 0, 0, 0];
            unsafe { std::ptr::copy_nonoverlapping(info.as_ptr(), data as *mut u32, info.len()) };
            LEN
        }
        _ => u32::MAX,
    }
}

/// A real mouse's or keyboard's report: no motion, no button, no key (the release of no key,
/// virtual key 0xFF).
unsafe fn hide_real(report: *mut u8) {
    unsafe {
        match (report as *const u32).read_unaligned() {
            RIM_TYPEMOUSE => std::ptr::write_bytes(report.add(HEADER), 0, RAWINPUT - HEADER),
            RIM_TYPEKEYBOARD => {
                // RAWKEYBOARD: MakeCode, Flags (1: break), Reserved, VKey, Message
                let kbd = report.add(HEADER) as *mut u16;
                kbd.write_unaligned(0);
                kbd.add(1).write_unaligned(1);
                kbd.add(3).write_unaligned(0xff);
                (report.add(HEADER + 8) as *mut u32).write_unaligned(WM_KEYUP);
            }
            _ => {}
        }
    }
}

/// The trackball's report: a relative mouse motion, the motion gathered since the last one.
unsafe extern "system" fn get_raw_input_data(input: P, command: u32, data: P, size: *mut u32, header_size: u32) -> u32 {
    if input as usize != REPORT {
        let orig: GetRawInputData = unsafe { std::mem::transmute(GET_RAW_INPUT_DATA.load(Ordering::Relaxed)) };
        let n = unsafe { orig(input, command, data, size, header_size) };
        if command == RID_INPUT && !data.is_null() && n != u32::MAX && n as usize >= RAWINPUT {
            unsafe { hide_real(data as *mut u8) };
        }
        return n;
    }
    if size.is_null() || header_size as usize != HEADER {
        return u32::MAX;
    }
    let len = match command {
        RID_HEADER => HEADER,
        RID_INPUT => RAWINPUT,
        _ => return u32::MAX,
    } as u32;
    if data.is_null() {
        unsafe { *size = len };
        return 0;
    }
    if unsafe { *size } < len {
        unsafe { SetLastError(ERROR_INSUFFICIENT_BUFFER) };
        return u32::MAX;
    }
    let mut report = [0u8; RAWINPUT];
    report[0..4].copy_from_slice(&RIM_TYPEMOUSE.to_le_bytes());
    report[4..8].copy_from_slice(&(RAWINPUT as u32).to_le_bytes());
    report[8..16].copy_from_slice(&DEVICE.to_le_bytes());
    if command == RID_INPUT {
        // RAWMOUSE: usFlags 0 (relative), lLastX, lLastY
        let x = MOTION[0].swap(0, Ordering::Relaxed);
        let y = MOTION[1].swap(0, Ordering::Relaxed);
        report[HEADER + 12..HEADER + 16].copy_from_slice(&x.to_le_bytes());
        report[HEADER + 16..HEADER + 20].copy_from_slice(&y.to_le_bytes());
        PENDING.store(false, Ordering::Relaxed);
    }
    unsafe { std::ptr::copy_nonoverlapping(report.as_ptr(), data as *mut u8, len as usize) };
    len
}
