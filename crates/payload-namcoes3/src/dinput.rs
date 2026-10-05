//! The game also enumerates DirectInput game controllers: their buttons act through its own
//! controls (R1 = view...) on top of the I/O board. `EnumDevices` of the interfaces it creates
//! only reports the keyboard and the mouse.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

type P = *mut c_void;
type EnumCallback = unsafe extern "system" fn(instance: *const u8, context: P) -> i32;
type EnumDevices = unsafe extern "system" fn(this: P, kind: u32, cb: EnumCallback, context: P, flags: u32) -> i32;
type Create = unsafe extern "system" fn(instance: P, version: u32, iid: P, out: *mut P, outer: P) -> i32;

const DI8DEVTYPE_MOUSE: u8 = 0x12;
const DI8DEVTYPE_KEYBOARD: u8 = 0x13;
/// `EnumDevices` in the IDirectInput8A / W vtables.
const ENUM_DEVICES_SLOT: usize = 4;

static CREATE: AtomicUsize = AtomicUsize::new(0);
/// (vtable, original EnumDevices): the A and W interfaces have their own.
static VTABLES: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

struct Filtered {
    cb: EnumCallback,
    context: P,
}

unsafe extern "system" fn filter(instance: *const u8, context: P) -> i32 {
    // DIDEVICEINSTANCE: dwSize, guidInstance, guidProduct, dwDevType (low byte: type)
    let kind = unsafe { *instance.add(36) };
    if kind != DI8DEVTYPE_MOUSE && kind != DI8DEVTYPE_KEYBOARD {
        return 1; // DIENUM_CONTINUE
    }
    let f = unsafe { &*(context as *const Filtered) };
    unsafe { (f.cb)(instance, f.context) }
}

unsafe extern "system" fn enum_devices(this: P, kind: u32, cb: EnumCallback, context: P, flags: u32) -> i32 {
    let vtable = unsafe { *(this as *const usize) };
    let original = VTABLES.lock().unwrap().iter().find(|(v, _)| *v == vtable).map(|(_, o)| *o);
    let Some(original) = original else { return 0x80004005u32 as i32 }; // E_FAIL
    let real: EnumDevices = unsafe { std::mem::transmute(original) };
    let filtered = Filtered { cb, context };
    unsafe { real(this, kind, filter, &filtered as *const Filtered as P, flags) }
}

unsafe extern "system" fn create(instance: P, version: u32, iid: P, out: *mut P, outer: P) -> i32 {
    let real: Create = unsafe { std::mem::transmute(CREATE.load(Ordering::Relaxed)) };
    let r = unsafe { real(instance, version, iid, out, outer) };
    if r == 0 && !out.is_null() && unsafe { !(*out).is_null() } {
        let vtable = unsafe { *(*out as *const usize) };
        let mut vtables = VTABLES.lock().unwrap();
        if !vtables.iter().any(|(v, _)| *v == vtable) {
            let slot = (vtable + ENUM_DEVICES_SLOT * size_of::<usize>()) as *mut usize;
            let mut old = 0;
            unsafe {
                vtables.push((vtable, slot.read()));
                VirtualProtect(slot.cast(), size_of::<usize>(), PAGE_READWRITE, &mut old);
                slot.write(enum_devices as *const () as usize);
                VirtualProtect(slot.cast(), size_of::<usize>(), old, &mut old);
            }
            log!("dinput: game controllers hidden from the game");
        }
    }
    r
}

pub(crate) fn init() {
    if let Some(o) = unsafe { iat::hook("dinput8.dll", "DirectInput8Create", create as *const () as usize) } {
        CREATE.store(o, Ordering::Relaxed);
    }
}
