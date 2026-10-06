//! DirectInput: Wine's real one, with the host's game controllers hidden.
//!
//! Re-Volt (setup at 0x44a990) does `DirectInputCreateA(hinst, 0x500, &di, NULL)`, creates the
//! system keyboard (c_dfDIKeyboard, polled every frame into 0x8eae68) and mouse, then
//! `EnumDevices(DIDEVTYPE_JOYSTICK, ...)` and appends every joystick found to its controller
//! list, followed by two built-in entries without a DirectInput device: "Driving Pod" (the
//! cabinet) and "TsuMo Joystick, Throttle". The controller index selects the pedal layout and
//! the cabinet reads the wheel, pedals and buttons through TsuInput GetJoyInfo
//! (`tsuinput.rs`) — never through DirectInput.
//!
//! A host joystick that Wine exposes would become controller 0 and take the steering away
//! from the virtual stick, so the joystick enumeration is answered empty: the game always
//! picks the "Driving Pod". The keyboard and the mouse stay Wine's.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

type P = *mut c_void;
type HRESULT = i32;

const DI_OK: HRESULT = 0;
/// IDirectInputA::EnumDevices
const SLOT_ENUM_DEVICES: usize = 4;
/// DIDEVTYPE_JOYSTICK (DirectInput 5)
const DIDEVTYPE_JOYSTICK: u32 = 4;

static ORIG_CREATE_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENUM_DEVICES: AtomicUsize = AtomicUsize::new(0);
/// The game's enumeration callback, while one `EnumDevices(all types)` call is forwarded.
static GAME_CALLBACK: AtomicUsize = AtomicUsize::new(0);

type CreateFn = unsafe extern "system" fn(P, u32, *mut P, P) -> HRESULT;
type EnumCallback = unsafe extern "system" fn(*const u8, P) -> i32;
type EnumDevicesFn = unsafe extern "system" fn(P, u32, EnumCallback, P, u32) -> HRESULT;

/// Forwards everything but joysticks/game controllers (dwDevType low byte >= 4) to the game.
unsafe extern "system" fn filter_callback(instance: *const u8, context: P) -> i32 {
    const DIENUM_CONTINUE: i32 = 1;
    // DIDEVICEINSTANCEA: dwSize, guidInstance, guidProduct, dwDevType (+0x24)
    let dev_type = unsafe { (instance.add(0x24) as *const u32).read_unaligned() };
    if dev_type & 0xff >= DIDEVTYPE_JOYSTICK {
        return DIENUM_CONTINUE;
    }
    let cb: EnumCallback = unsafe { std::mem::transmute(GAME_CALLBACK.load(Ordering::Relaxed)) };
    unsafe { cb(instance, context) }
}

/// `IDirectInputA::EnumDevices(dwDevType, callback, pvRef, dwFlags)` without game controllers.
unsafe extern "system" fn enum_devices(this: P, dev_type: u32, callback: EnumCallback, context: P, flags: u32) -> HRESULT {
    if dev_type >= DIDEVTYPE_JOYSTICK {
        log!("dinput: EnumDevices(type {dev_type}) -> none (cabinet controls come from TsuInput)");
        return DI_OK;
    }
    let orig: EnumDevicesFn = unsafe { std::mem::transmute(ORIG_ENUM_DEVICES.load(Ordering::Relaxed)) };
    GAME_CALLBACK.store(callback as usize, Ordering::Relaxed);
    unsafe { orig(this, dev_type, filter_callback, context, flags) }
}

/// Patch `EnumDevices` in the (Wine-wide) IDirectInputA vtable of `di`.
unsafe fn hook_enum_devices(di: P) {
    let slot = unsafe { (*(di as *const *mut usize)).add(SLOT_ENUM_DEVICES) };
    let cur = unsafe { slot.read() };
    if cur == enum_devices as *const () as usize {
        return;
    }
    ORIG_ENUM_DEVICES.store(cur, Ordering::Relaxed);
    unsafe {
        let mut old = 0;
        VirtualProtect(slot.cast(), size_of::<usize>(), PAGE_READWRITE, &mut old);
        slot.write(enum_devices as *const () as usize);
        VirtualProtect(slot.cast(), size_of::<usize>(), old, &mut old);
    }
}

/// `DirectInputCreateA(hinst, version, out, outer)`: Wine's object, joysticks hidden.
unsafe extern "system" fn create(instance: P, version: u32, out: *mut P, outer: P) -> HRESULT {
    let orig: CreateFn = unsafe { std::mem::transmute(ORIG_CREATE_A.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(instance, version, out, outer) };
    if hr >= 0 && !out.is_null() && unsafe { !(*out).is_null() } {
        unsafe { hook_enum_devices(*out) };
        log!("dinput: DirectInputCreateA({version:#x}) -> Wine's, game controllers hidden");
    } else {
        log!("dinput: DirectInputCreateA({version:#x}) failed {hr:#x}");
    }
    hr
}

pub fn init() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("dinput.dll", "DirectInputCreateA", create as *const () as usize) } {
        ORIG_CREATE_A.store(o, Ordering::Relaxed);
        log!("dinput: DirectInputCreateA hooked (host game controllers hidden)");
    } else {
        log!("dinput: no dinput.dll!DirectInputCreateA in the game IAT to hook");
    }
}
