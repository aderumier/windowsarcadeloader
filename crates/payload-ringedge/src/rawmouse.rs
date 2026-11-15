//! A raw input mouse of the cabinet's (`WAL_RINGEDGE_RAW_MOUSE`: its device name's USB id,
//! Border Break Scramble's aim control `HID#Vid_0ca3&Pid_000b`): the game counts the raw input
//! mice whose name holds it at boot (FUN_0067cdf0, case sensitive: Wine's names are upper
//! case), its error without one (a develop keychip waives it). Its motion and buttons come
//! through DirectInput's system mouse ([`wal_payload_common::dinput`]'s stick mouse): the device
//! is only listed.
//!
//! The game executable's imports of `GetRawInputDeviceList` / `GetRawInputDeviceInfoA` hooked:
//! the real devices, then this mouse (a fake handle), its name and mouse information.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::SetLastError;

type P = *mut c_void;

/// The mouse's device handle (odd: never a real one).
const DEVICE: usize = 0x5245_2001;
const RIM_TYPEMOUSE: u32 = 0;
const RIDI_DEVICENAME: u32 = 0x2000_0007;
const RIDI_DEVICEINFO: u32 = 0x2000_000b;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
/// RAWINPUTDEVICELIST (32-bit): device handle, type.
const ENTRY: usize = 8;

static GET_DEVICE_LIST: AtomicUsize = AtomicUsize::new(0);
static GET_DEVICE_INFO: AtomicUsize = AtomicUsize::new(0);
static NAME: OnceLock<Vec<u8>> = OnceLock::new();

type GetDeviceList = unsafe extern "system" fn(P, *mut u32, u32) -> u32;
type GetDeviceInfo = unsafe extern "system" fn(P, u32, P, *mut u32) -> u32;

pub(crate) fn init() {
    let Ok(id) = std::env::var("WAL_RINGEDGE_RAW_MOUSE") else { return };
    let name = format!("\\\\?\\{}#7&5e9a&0&0000#{{378de44c-56ef-11d1-bc8c-00a0c91405dd}}", id.trim());
    log!("rawmouse: {name}");
    let _ = NAME.set(name.into_bytes().into_iter().chain(Some(0)).collect());
    let hooks: [(&str, usize, &AtomicUsize); 2] = [
        ("GetRawInputDeviceList", get_device_list as *const () as usize, &GET_DEVICE_LIST),
        ("GetRawInputDeviceInfoA", get_device_info as *const () as usize, &GET_DEVICE_INFO),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook("user32.dll", name, hook) } {
            Some(o) => orig.store(o, Ordering::Relaxed),
            None => log!("rawmouse: user32!{name} not imported"),
        }
    }
}

/// The real devices, then the mouse.
unsafe extern "system" fn get_device_list(list: P, count: *mut u32, size: u32) -> u32 {
    let orig: GetDeviceList = unsafe { std::mem::transmute(GET_DEVICE_LIST.load(Ordering::Relaxed)) };
    if count.is_null() || size as usize != ENTRY {
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
    let n = if real == 0 { 0 } else { unsafe { orig(list, &mut room, size) } };
    if n == u32::MAX {
        return n;
    }
    unsafe {
        let entry = (list as *mut u8).add(n as usize * ENTRY);
        (entry as *mut u32).write_unaligned(DEVICE as u32);
        (entry.add(4) as *mut u32).write_unaligned(RIM_TYPEMOUSE);
    }
    n + 1
}

/// The mouse's name and mouse information (an id, 3 buttons).
unsafe extern "system" fn get_device_info(device: P, command: u32, data: P, size: *mut u32) -> u32 {
    if device as usize != DEVICE {
        let orig: GetDeviceInfo = unsafe { std::mem::transmute(GET_DEVICE_INFO.load(Ordering::Relaxed)) };
        return unsafe { orig(device, command, data, size) };
    }
    if size.is_null() {
        return u32::MAX;
    }
    let reply: Vec<u8> = match command {
        RIDI_DEVICENAME => NAME.get().cloned().unwrap_or_default(),
        // RID_DEVICE_INFO: cbSize, dwType, RID_DEVICE_INFO_MOUSE (id, buttons, sample rate,
        // horizontal wheel), the union's size
        RIDI_DEVICEINFO => [32, RIM_TYPEMOUSE, 0x100, 3, 0, 0, 0, 0].iter().flat_map(|v: &u32| v.to_le_bytes()).collect(),
        _ => return u32::MAX,
    };
    let len = reply.len() as u32;
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
    unsafe { std::ptr::copy_nonoverlapping(reply.as_ptr(), data as *mut u8, reply.len()) };
    len
}
