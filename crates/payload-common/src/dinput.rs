//! `WAL_DINPUT_DISABLE=1`: the game gets a DirectInput that works but sees no input, so only
//! its I/O board drives it. Some arcade builds still read the keyboard and joysticks through
//! DirectInput (debug or operator keys): Raiden IV (Type X) enters its test menu on the PC
//! keyboard's `2`, which the launcher also maps to P2 start (the keyboard is not grabbed, the
//! game window sees it).
//!
//! `DirectInputCreateA/W/Ex` and `DirectInput8Create` return a static fake interface (the
//! IDirectInput8 vtable, a superset of the older ones): `EnumDevices` lists nothing,
//! `CreateDevice` returns a fake device whose methods succeed, with zeroed states and no
//! buffered data. Failing the creation instead would end some games.
//!
//! `WAL_KEYBOARD_STATE_DISABLE=1`: the game's `user32!GetKeyboardState` reports no key pressed
//! (Wacky Races reads START on the PC keyboard's Enter and VIEW on Right Shift that way, next
//! to its JVS switches).

use std::ffi::c_void;

use crate::{iat, log};

type P = *mut c_void;

const DI_OK: i32 = 0;
const DIERR_UNSUPPORTED: i32 = 0x80004001u32 as i32;
const DIERR_DEVICENOTREG: i32 = 0x80040154u32 as i32;

#[repr(C)]
struct Object {
    vtable: *const *const (),
}
unsafe impl Sync for Object {}

struct VTable<const N: usize>([*const (); N]);
unsafe impl<const N: usize> Sync for VTable<N> {}

// Methods by argument count after `this` (stdcall: the callee pops them).
unsafe extern "system" fn ok0(_this: P) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok1(_this: P, _a: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok2(_this: P, _a: usize, _b: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok3(_this: P, _a: usize, _b: usize, _c: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok4(_this: P, _a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn ok5(_this: P, _a: usize, _b: usize, _c: usize, _d: usize, _e: usize) -> i32 {
    DI_OK
}
unsafe extern "system" fn unsupported1(_this: P, _a: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn unsupported2(_this: P, _a: usize, _b: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn unsupported3(_this: P, _a: usize, _b: usize, _c: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn unsupported4(_this: P, _a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    DIERR_UNSUPPORTED
}
unsafe extern "system" fn ref_count(_this: P) -> u32 {
    1
}

/// `QueryInterface`: any interface of the object is the object itself (IDirectInputDevice2/7/8
/// share the first slots).
unsafe extern "system" fn query_interface(this: P, _iid: P, out: *mut P) -> i32 {
    if !out.is_null() {
        unsafe { *out = this };
    }
    DI_OK
}

/// `GetDeviceState(cbData, lpvData)`: nothing pressed, axes 0.
unsafe extern "system" fn get_device_state(_this: P, size: u32, data: *mut u8) -> i32 {
    if !data.is_null() {
        unsafe { std::ptr::write_bytes(data, 0, size as usize) };
    }
    DI_OK
}

/// `GetDeviceData(cbObjectData, rgdod, pdwInOut, dwFlags)`: no buffered events.
unsafe extern "system" fn get_device_data(_this: P, _size: u32, _data: P, count: *mut u32, _flags: u32) -> i32 {
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    DI_OK
}

/// IDirectInputDevice8 (IDirectInputDeviceA/2/7 are its prefixes).
static DEVICE_VTABLE: VTable<32> = VTable([
    query_interface as *const (),
    ref_count as *const (), // AddRef
    ref_count as *const (), // Release
    ok1 as *const (), // GetCapabilities
    ok3 as *const (), // EnumObjects
    ok2 as *const (), // GetProperty
    ok2 as *const (), // SetProperty
    ok0 as *const (), // Acquire
    ok0 as *const (), // Unacquire
    get_device_state as *const (),
    get_device_data as *const (),
    ok1 as *const (), // SetDataFormat
    ok1 as *const (), // SetEventNotification
    ok2 as *const (), // SetCooperativeLevel
    unsupported3 as *const (), // GetObjectInfo
    unsupported1 as *const (), // GetDeviceInfo
    ok2 as *const (), // RunControlPanel
    ok3 as *const (), // Initialize
    unsupported4 as *const (), // CreateEffect
    ok3 as *const (), // EnumEffects
    unsupported2 as *const (), // GetEffectInfo
    unsupported1 as *const (), // GetForceFeedbackState
    ok1 as *const (), // SendForceFeedbackCommand
    ok3 as *const (), // EnumCreatedEffectObjects
    unsupported1 as *const (), // Escape
    ok0 as *const (), // Poll
    ok4 as *const (), // SendDeviceData
    ok4 as *const (), // EnumEffectsInFile
    ok4 as *const (), // WriteEffectToFile
    unsupported3 as *const (), // BuildActionMap
    unsupported3 as *const (), // SetActionMap
    unsupported1 as *const (), // GetImageInfo
]);
static DEVICE: Object = Object { vtable: DEVICE_VTABLE.0.as_ptr() };

/// `CreateDevice(rguid, lplpDirectInputDevice, pUnkOuter)`
unsafe extern "system" fn create_device(_this: P, _guid: P, out: *mut P, _outer: P) -> i32 {
    if !out.is_null() {
        unsafe { *out = &DEVICE as *const Object as P };
    }
    DI_OK
}

/// IDirectInput8 (IDirectInputA/2/7 share the first 8 slots; IDirectInput7's CreateDeviceEx
/// sits where FindDevice is, with one more argument: not used by these games).
static DINPUT_VTABLE: VTable<11> = VTable([
    query_interface as *const (),
    ref_count as *const (), // AddRef
    ref_count as *const (), // Release
    create_device as *const (),
    ok4 as *const (), // EnumDevices: none
    unsupported1 as *const (), // GetDeviceStatus
    ok2 as *const (), // RunControlPanel
    ok2 as *const (), // Initialize
    unsupported3 as *const (), // FindDevice
    ok5 as *const (), // EnumDevicesBySemantics
    unsupported4 as *const (), // ConfigureDevices
]);
static DINPUT: Object = Object { vtable: DINPUT_VTABLE.0.as_ptr() };

fn fake(out: *mut P) -> i32 {
    if out.is_null() {
        return DIERR_DEVICENOTREG;
    }
    unsafe { *out = &DINPUT as *const Object as P };
    log!("dinput: game got the fake DirectInput");
    DI_OK
}

/// `DirectInputCreateA/W(hinst, version, out, outer)`
unsafe extern "system" fn create(_instance: P, _version: u32, out: *mut P, _outer: P) -> i32 {
    fake(out)
}

/// `DirectInputCreateEx` / `DirectInput8Create(hinst, version, riid, out, outer)`
unsafe extern "system" fn create_iid(_instance: P, _version: u32, _iid: P, out: *mut P, _outer: P) -> i32 {
    fake(out)
}

unsafe extern "system" fn get_keyboard_state(keys: *mut u8) -> i32 {
    if !keys.is_null() {
        unsafe { std::ptr::write_bytes(keys, 0, 256) };
    }
    1
}

pub fn init() {
    if std::env::var("WAL_KEYBOARD_STATE_DISABLE").is_ok_and(|v| v.trim() == "1") {
        unsafe { iat::hook("user32.dll", "GetKeyboardState", get_keyboard_state as *const () as usize) };
        log!("dinput: GetKeyboardState reports no key");
    }
    if !std::env::var("WAL_DINPUT_DISABLE").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    unsafe {
        for name in ["DirectInputCreateA", "DirectInputCreateW"] {
            iat::hook("dinput.dll", name, create as *const () as usize);
        }
        iat::hook("dinput.dll", "DirectInputCreateEx", create_iid as *const () as usize);
        iat::hook("dinput8.dll", "DirectInput8Create", create_iid as *const () as usize);
    }
    log!("dinput: disabled (fake DirectInput without devices)");
}
