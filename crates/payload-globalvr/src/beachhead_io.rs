//! Beach Head 2000's cabinet controls, for its frontend (`ShellControl.ocx`, loaded by
//! `UniverShell.exe`) and its game (`Beach16.exe`), which both read them the same way:
//!
//! * the buttons: joystick 0's (`joyGetPosEx`), the cabinet's I/O board seen as a joystick;
//! * the gun's motion: DirectInput's system mouse (relative X and Y), the turret's encoders. The
//!   game turns the turret by it, the frontend moves its selections;
//! * the coins: the cabinet's coin driver counted them in the registry, `HKLM\Software\GVRShell`
//!   `ThisCoinCounter` (`CoinAvailable` = this - `LastCoinCounter`, the credits the frontend and the
//!   game's continues use up). Counted by the frontend's payload (it runs during the games too).
//!
//! The real keyboard, mouse and joysticks are hidden: window input messages dropped, the
//! DirectInput devices' states replaced.
//!
//! Buttons (joystick bits; the frontend's names, from its `Operator\Buttons` registry):
//!
//! | bit | game | frontend |
//! |---|---|---|
//! | 0x1 | air strike | attract advance |
//! | 0x2 | start | accept |
//! | 0x4 | cannon | back |
//! | 0x8 | machine gun | |
//! | 0x20 | | operator menu |

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log, mapping::ButtonMap};
use wal_protocol::{Axis, button};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExA,
    RegQueryValueExA, RegSetValueExA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, WM_KEYFIRST, WM_KEYLAST, WM_MOUSEFIRST, WM_MOUSELAST, WM_NULL};

type P = *mut c_void;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Native(u32);

const NATIVES: &[(&str, Native)] = &[
    ("airstrike", Native(0x1)),
    ("start", Native(0x2)),
    ("cannon", Native(0x4)),
    ("machinegun", Native(0x8)),
    ("operator", Native(0x20)),
];

/// Virtual stick -> cabinet, overridden by the profile `native_map`.
/// The trigger is also the frontend's accept ("FIRE - execute selection", the game unaffected).
const DEFAULT_MAP: &[(&str, &str)] =
    &[("b1", "machinegun+start"), ("b2", "cannon"), ("b3", "airstrike"), ("start", "start"), ("test", "operator")];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(u32::MAX);

fn buttons() -> u32 {
    let stick = wal_payload_common::input(0);
    let mut bits = 0;
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP)).for_each_pressed(&stick, |Native(b)| bits |= b);
    if LAST_BUTTONS.swap(bits, Ordering::Relaxed) != bits {
        log!("beachhead: buttons {bits:#x}");
    }
    bits
}

/// `joyGetPosEx(id, JOYINFOEX *)`: joystick 0 is the board (axes centered, its buttons).
unsafe extern "system" fn joy_get_pos_ex(id: u32, info: *mut u32) -> u32 {
    const JOYERR_UNPLUGGED: u32 = 167;
    if id != 0 || info.is_null() {
        return JOYERR_UNPLUGGED;
    }
    let b = buttons();
    unsafe {
        // dwXpos..dwVpos, dwButtons, dwButtonNumber, dwPOV
        for i in 2..8 {
            *info.add(i) = 0x7fff;
        }
        *info.add(8) = b;
        *info.add(9) = b.count_ones();
        *info.add(10) = 0xffff;
    }
    0
}

/// The gun's motion at full tilt, counts per second (`WAL_BEACHHEAD_AIM_SPEED`; a full turn of
/// the turret is 2048). By the time since the device's last read: the game reads every frame,
/// the frontend every 100 ms (its trackball code ignores reads under 20 counts).
fn aim_speed() -> f32 {
    static SPEED: OnceLock<f32> = OnceLock::new();
    *SPEED.get_or_init(|| std::env::var("WAL_BEACHHEAD_AIM_SPEED").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(720.0))
}

/// A system mouse device: its last read and the sub-count remainders of X and Y (slow tilts still
/// move).
struct Mouse {
    device: usize,
    last: Option<std::time::Instant>,
    rest: [f32; 2],
}

/// The system mouse devices (one per shell control: the operator menu's frames have several).
static MICE: std::sync::Mutex<Vec<Mouse>> = std::sync::Mutex::new(Vec::new());

/// One axis' motion over `seconds`: the tilt times the speed, the remainder kept.
fn motion(axis: i16, seconds: f32, rest: &mut f32) -> i32 {
    let total = axis as f32 / 32767.0 * aim_speed() * seconds + *rest;
    let counts = total.trunc();
    *rest = if axis == 0 { 0.0 } else { total - counts };
    counts as i32
}

/// The motion of `device` since its last read, `None` when it is not a system mouse.
fn mouse_motion(device: P) -> Option<(i32, i32)> {
    let mut mice = MICE.lock().ok()?;
    let mouse = mice.iter_mut().find(|m| m.device == device as usize)?;
    let now = std::time::Instant::now();
    let seconds = mouse.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.25));
    mouse.last = Some(now);
    let stick = wal_payload_common::input(0);
    let [rx, ry] = &mut mouse.rest;
    Some((motion(stick.axis(Axis::LeftX), seconds, rx), motion(stick.axis(Axis::LeftY), seconds, ry)))
}

/// GUID_SysMouse's first field ({6F1D2B60-D5A0-11CF-BFC7-444553540000}).
const SYS_MOUSE: u32 = 0x6F1D_2B60;
/// IDirectInput8::CreateDevice, IDirectInputDevice8::GetDeviceState / GetDeviceData slots.
const CREATE_DEVICE: usize = 3;
const GET_DEVICE_STATE: usize = 9;
const GET_DEVICE_DATA: usize = 10;

static ORIG_CREATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_STATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_DATA: AtomicUsize = AtomicUsize::new(0);


/// Replaces a vtable slot, returns the previous function.
unsafe fn patch_slot(object: P, slot: usize, replacement: usize) -> usize {
    unsafe {
        let entry = (*(object as *const *mut usize)).add(slot);
        let mut old = 0;
        VirtualProtect(entry as _, size_of::<usize>(), PAGE_READWRITE, &mut old);
        let previous = entry.read();
        entry.write(replacement);
        VirtualProtect(entry as _, size_of::<usize>(), old, &mut old);
        previous
    }
}

/// `DirectInput8Create(hinst, version, riid, out, outer)`: the real DirectInput, its devices'
/// states replaced.
unsafe extern "system" fn di8_create(instance: P, version: u32, iid: P, out: *mut P, outer: P) -> i32 {
    let f: unsafe extern "system" fn(P, u32, P, *mut P, P) -> i32 = unsafe { std::mem::transmute(ORIG_CREATE.load(Ordering::Relaxed)) };
    let hr = unsafe { f(instance, version, iid, out, outer) };
    if hr >= 0 && !out.is_null() && unsafe { !(*out).is_null() } && ORIG_CREATE_DEVICE.load(Ordering::Relaxed) == 0 {
        ORIG_CREATE_DEVICE.store(unsafe { patch_slot(*out, CREATE_DEVICE, create_device as *const () as usize) }, Ordering::Relaxed);
    }
    hr
}

/// `IDirectInput8::CreateDevice(this, guid, out, outer)`
unsafe extern "system" fn create_device(this: P, guid: *const u32, out: *mut P, outer: P) -> i32 {
    let f: unsafe extern "system" fn(P, *const u32, *mut P, P) -> i32 =
        unsafe { std::mem::transmute(ORIG_CREATE_DEVICE.load(Ordering::Relaxed)) };
    let hr = unsafe { f(this, guid, out, outer) };
    if hr >= 0 && !out.is_null() && unsafe { !(*out).is_null() } {
        let device = unsafe { *out };
        if !guid.is_null() && unsafe { *guid } == SYS_MOUSE {
            if let Ok(mut mice) = MICE.lock() {
                mice.push(Mouse { device: device as usize, last: None, rest: [0.0; 2] });
            }
            log!("beachhead: the gun's motion on the system mouse");
        }
        // the device vtable is shared: patched once, the device checked
        if ORIG_GET_STATE.load(Ordering::Relaxed) == 0 {
            ORIG_GET_STATE.store(unsafe { patch_slot(device, GET_DEVICE_STATE, get_device_state as *const () as usize) }, Ordering::Relaxed);
            ORIG_GET_DATA.store(unsafe { patch_slot(device, GET_DEVICE_DATA, get_device_data as *const () as usize) }, Ordering::Relaxed);
        }
    }
    hr
}

/// `GetDeviceState(this, size, data)`: the system mouse moves by the virtual stick (DIMOUSESTATE:
/// X, Y, Z, buttons), the other devices see nothing.
unsafe extern "system" fn get_device_state(this: P, size: u32, data: *mut u8) -> i32 {
    let f: unsafe extern "system" fn(P, u32, *mut u8) -> i32 = unsafe { std::mem::transmute(ORIG_GET_STATE.load(Ordering::Relaxed)) };
    let hr = unsafe { f(this, size, data) };
    if data.is_null() {
        return hr;
    }
    unsafe { std::ptr::write_bytes(data, 0, size as usize) };
    if size >= 8
        && let Some((x, y)) = mouse_motion(this)
    {
        unsafe {
            let xy = data as *mut i32;
            *xy = x;
            *xy.add(1) = y;
        }
        return 0;
    }
    hr
}

/// `GetDeviceData(this, size, data, count, flags)`: no buffered input.
unsafe extern "system" fn get_device_data(this: P, size: u32, data: P, count: *mut u32, flags: u32) -> i32 {
    let f: unsafe extern "system" fn(P, u32, P, *mut u32, u32) -> i32 = unsafe { std::mem::transmute(ORIG_GET_DATA.load(Ordering::Relaxed)) };
    let hr = unsafe { f(this, size, data, count, flags) };
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    hr
}

static ORIG_PEEK: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET: AtomicUsize = AtomicUsize::new(0);

/// A keyboard or mouse message of the real devices becomes `WM_NULL`.
fn filter(msg: *mut MSG) {
    if let Some(m) = unsafe { msg.as_mut() }
        && ((WM_KEYFIRST..=WM_KEYLAST).contains(&m.message) || (WM_MOUSEFIRST + 1..=WM_MOUSELAST).contains(&m.message))
    {
        m.message = WM_NULL;
    }
}

unsafe extern "system" fn peek_message(msg: *mut MSG, hwnd: P, min: u32, max: u32, remove: u32) -> i32 {
    let f: unsafe extern "system" fn(*mut MSG, P, u32, u32, u32) -> i32 = unsafe { std::mem::transmute(ORIG_PEEK.load(Ordering::Relaxed)) };
    let r = unsafe { f(msg, hwnd, min, max, remove) };
    if r != 0 {
        filter(msg);
    }
    r
}

unsafe extern "system" fn get_message(msg: *mut MSG, hwnd: P, min: u32, max: u32) -> i32 {
    let f: unsafe extern "system" fn(*mut MSG, P, u32, u32) -> i32 = unsafe { std::mem::transmute(ORIG_GET.load(Ordering::Relaxed)) };
    let r = unsafe { f(msg, hwnd, min, max) };
    if r > 0 {
        filter(msg);
    }
    r
}

/// `GetAsyncKeyState` / `GetKeyState`: no key of the real keyboard.
unsafe extern "system" fn no_key(_vk: i32) -> i16 {
    0
}

/// The controls of the module at `base` (the game executable, the frontend's control).
pub(crate) fn hook(base: usize) {
    unsafe {
        iat::hook_module(base, "winmm.dll", "joyGetPosEx", joy_get_pos_ex as *const () as usize);
        if let Some(o) = iat::hook_module(base, "dinput8.dll", "DirectInput8Create", di8_create as *const () as usize)
            && o != di8_create as *const () as usize
        {
            ORIG_CREATE.store(o, Ordering::Relaxed);
        }
        iat::hook_module(base, "user32.dll", "GetAsyncKeyState", no_key as *const () as usize);
        iat::hook_module(base, "user32.dll", "GetKeyState", no_key as *const () as usize);
    }
}

/// The real keyboard and mouse messages of the executable dropped.
pub(crate) fn filter_messages() {
    unsafe {
        if let Some(o) = iat::hook("user32.dll", "PeekMessageA", peek_message as *const () as usize) {
            ORIG_PEEK.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("user32.dll", "GetMessageA", get_message as *const () as usize) {
            ORIG_GET.store(o, Ordering::Relaxed);
        }
    }
}

fn reg_key() -> Option<HKEY> {
    let mut key: HKEY = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExA(
            HKEY_LOCAL_MACHINE,
            c"Software\\GVRShell".as_ptr().cast(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    (status == 0).then_some(key)
}

fn reg_get(key: HKEY, name: &std::ffi::CStr) -> u32 {
    let (mut value, mut size) = (0u32, 4u32);
    let status =
        unsafe { RegQueryValueExA(key, name.as_ptr().cast(), std::ptr::null(), std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size) };
    if status == 0 { value } else { 0 }
}

fn reg_set(key: HKEY, name: &std::ffi::CStr, value: u32) {
    unsafe { RegSetValueExA(key, name.as_ptr().cast(), 0, REG_DWORD, (&value as *const u32).cast(), 4) };
}

/// A coin: the coin driver's counters.
fn coin() {
    let Some(key) = reg_key() else {
        return;
    };
    let this = reg_get(key, c"ThisCoinCounter").wrapping_add(1) & 0xffff;
    let last = reg_get(key, c"LastCoinCounter");
    reg_set(key, c"ThisCoinCounter", this);
    reg_set(key, c"CoinAvailable", this.wrapping_sub(last) & 0xffff);
    unsafe { RegCloseKey(key) };
    log!("beachhead: coin ({} available)", this.wrapping_sub(last) & 0xffff);
}

/// The coin button watched (in the frontend, running for the whole session).
pub(crate) fn watch_coins() {
    std::thread::spawn(|| {
        let mut held = false;
        loop {
            let down = (0..2).any(|p| wal_payload_common::input(p).pressed(button::COIN));
            if down && !held {
                coin();
            }
            held = down;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_keeps_fractions() {
        let mut rest = 0.0;
        // a quarter tilt for 8 reads of a frame: 720 / 4 / 60 = 3 counts each
        let moved: i32 = (0..8).map(|_| motion(32767 / 4 + 1, 1.0 / 60.0, &mut rest)).sum();
        assert_eq!(moved, 24);
        assert_eq!(motion(0, 1.0 / 60.0, &mut rest), 0);
        assert_eq!(rest, 0.0);
    }
}
