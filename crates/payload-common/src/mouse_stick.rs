//! `WAL_DINPUT_MOUSE_STICK=<speed>`: player 1's left stick moves DirectInput's system mouse
//! too, next to the real mouse (Super Monkey Ball Ticket Blitz's trackball is the system
//! mouse, read with `GetDeviceState`). The game keeps the real DirectInput: its system mouse
//! device gets a wrapped `GetDeviceState` that adds the stick's motion to the relative X and
//! Y axes, `<speed>` counts per poll at full tilt (the game polls once a frame).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use wal_protocol::Axis;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

use crate::{iat, log};

type P = *mut c_void;

/// GUID_SysMouse's first field ({6F1D2B60-D5A0-11CF-BFC7-444553540000}).
const SYS_MOUSE: u32 = 0x6F1D_2B60;
/// IDirectInput8::CreateDevice, IDirectInputDevice8::GetDeviceState vtable slots.
const CREATE_DEVICE: usize = 3;
const GET_DEVICE_STATE: usize = 9;

static CREATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_STATE: AtomicUsize = AtomicUsize::new(0);
/// The game's system mouse device.
static MOUSE: AtomicUsize = AtomicUsize::new(0);
/// Counts per poll at full tilt (f32 bits).
static SPEED: AtomicU32 = AtomicU32::new(0);
/// Sub-count remainders of X and Y (f32 bits): slow tilts still move.
static REST: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Replaces a vtable slot, returns the previous function.
unsafe fn patch_slot(object: P, slot: usize, replacement: usize) -> usize {
    unsafe {
        let entry = (*(object as *const *mut usize)).add(slot);
        let mut old = 0;
        VirtualProtect(entry as _, std::mem::size_of::<usize>(), PAGE_READWRITE, &mut old);
        let previous = entry.read();
        entry.write(replacement);
        VirtualProtect(entry as _, std::mem::size_of::<usize>(), old, &mut old);
        previous
    }
}

/// `DirectInput8Create(hinst, version, riid, out, outer)`
unsafe extern "system" fn create(instance: P, version: u32, iid: P, out: *mut P, outer: P) -> i32 {
    let f: unsafe extern "system" fn(P, u32, P, *mut P, P) -> i32 = unsafe { std::mem::transmute(CREATE.load(Ordering::Relaxed)) };
    let hr = unsafe { f(instance, version, iid, out, outer) };
    if hr >= 0 && !out.is_null() && unsafe { !(*out).is_null() } && ORIG_CREATE_DEVICE.load(Ordering::Relaxed) == 0 {
        let previous = unsafe { patch_slot(*out, CREATE_DEVICE, create_device as *const () as usize) };
        ORIG_CREATE_DEVICE.store(previous, Ordering::Relaxed);
    }
    hr
}

/// `IDirectInput8::CreateDevice(this, guid, out, outer)`
unsafe extern "system" fn create_device(this: P, guid: *const u32, out: *mut P, outer: P) -> i32 {
    let f: unsafe extern "system" fn(P, *const u32, *mut P, P) -> i32 = unsafe { std::mem::transmute(ORIG_CREATE_DEVICE.load(Ordering::Relaxed)) };
    let hr = unsafe { f(this, guid, out, outer) };
    if hr >= 0 && !guid.is_null() && unsafe { *guid } == SYS_MOUSE && !out.is_null() && unsafe { !(*out).is_null() } {
        let device = unsafe { *out };
        MOUSE.store(device as usize, Ordering::Relaxed);
        // the vtable may be shared with the other devices: the wrapper checks the device
        if ORIG_GET_STATE.load(Ordering::Relaxed) == 0 {
            let previous = unsafe { patch_slot(device, GET_DEVICE_STATE, get_device_state as *const () as usize) };
            ORIG_GET_STATE.store(previous, Ordering::Relaxed);
        }
        log!("mouse-stick: system mouse device {device:?}");
    }
    hr
}

/// One axis' motion this poll: the tilt times the speed, the remainder kept.
fn motion(axis: i16, rest: &AtomicU32) -> i32 {
    let speed = f32::from_bits(SPEED.load(Ordering::Relaxed));
    let total = axis as f32 / 32767.0 * speed + f32::from_bits(rest.load(Ordering::Relaxed));
    let counts = total.trunc();
    rest.store(if axis == 0 { 0f32 } else { total - counts }.to_bits(), Ordering::Relaxed);
    counts as i32
}

/// `IDirectInputDevice8::GetDeviceState(this, size, data)`: DIMOUSESTATE starts with the
/// relative X and Y motions (LONG).
unsafe extern "system" fn get_device_state(this: P, size: u32, data: *mut i32) -> i32 {
    let f: unsafe extern "system" fn(P, u32, *mut i32) -> i32 = unsafe { std::mem::transmute(ORIG_GET_STATE.load(Ordering::Relaxed)) };
    let hr = unsafe { f(this, size, data) };
    if hr >= 0 && this as usize == MOUSE.load(Ordering::Relaxed) && size >= 8 && !data.is_null() {
        let stick = crate::input(0);
        unsafe {
            *data += motion(stick.axis(Axis::LeftX), &REST[0]);
            *data.add(1) += motion(stick.axis(Axis::LeftY), &REST[1]);
        }
    }
    hr
}

pub fn init() {
    let Some(speed) = std::env::var("WAL_DINPUT_MOUSE_STICK").ok().and_then(|v| v.trim().parse::<f32>().ok()) else { return };
    SPEED.store(speed.to_bits(), Ordering::Relaxed);
    match unsafe { iat::hook("dinput8.dll", "DirectInput8Create", create as *const () as usize) } {
        Some(original) => {
            CREATE.store(original, Ordering::Relaxed);
            log!("mouse-stick: left stick on the system mouse, {speed} counts per poll");
        }
        None => log!("mouse-stick: the game does not import DirectInput8Create"),
    }
}
