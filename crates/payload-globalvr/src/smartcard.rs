//! Need for Speed Carbon's card reader: the smart card slot of the cabinet frontend.
//!
//! GVR's data engine (`PLUSDE.dll`) reads the cards through `GVRStorageDevice.dll`, which loads
//! the reader's module by name (`LoadLibraryA("GVRSCR28.dll")`, the SCR28 reader over PC/SC) and
//! asks it `CreateGVRStorageDeviceImp()`: an object whose first field is its methods' table
//! (thiscall), then a 1024-byte error text the wrapper copies. Without a reader the frontend
//! waits 30 s on its startup check, its login screens take the career number only.
//!
//! The frontend's loads of its plug-ins are hooked, then the data engine's (it delay-loads
//! `GVRStorageDevice.dll` at its first card call): once the wrapper is loaded, its own
//! `LoadLibraryA("GVRSCR28.dll")` gets this DLL, whose `CreateGVRStorageDeviceImp` export
//! answers a reader with one card:
//!
//! * the `card` control inserts the card, the next press removes it; the frontend asks it out
//!   after reading its id: it comes out by itself 1.5 s after that read;
//! * the card is its id only (`WAL_GLOBALVR_CARD_ID`, a decimal number, default 1000000001):
//!   at the Career login, a card not linked yet asks a career number (and a name for a new
//!   career), linked to the card in the cabinet's database; the card logs it in from then on;
//! * its 8 KB memory reads zeros, written in memory only.

use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleHandleA, GetModuleHandleExW};

const OK: i32 = 0;
const NOT_CONNECTED: i32 = 4002;
const ERROR: i32 = 5000;
const STATUS_INSERTED: i32 = 1;
const STATUS_EMPTY: i32 = 2;
const CARD_SIZE: usize = 0x2000;
const DEFAULT_ID: i64 = 1_000_000_001;

static ORIG_PLAYER_LOAD: AtomicUsize = AtomicUsize::new(0);
static ORIG_PLAYER_LOAD_EX: AtomicUsize = AtomicUsize::new(0);
static ORIG_PLUSDE_LOAD: AtomicUsize = AtomicUsize::new(0);
static ORIG_WRAPPER_LOAD: AtomicUsize = AtomicUsize::new(0);
static INSERTED: AtomicBool = AtomicBool::new(false);
static MEMORY: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// When the frontend read the inserted card's id.
static READ_AT: Mutex<Option<Instant>> = Mutex::new(None);
const EJECT_AFTER_READ: Duration = Duration::from_millis(1500);

type LoadLibraryA = unsafe extern "system" fn(*const u8) -> usize;
type LoadLibraryExA = unsafe extern "system" fn(*const u8, usize, u32) -> usize;

/// In the frontend: its plug-in loads (`LoadLibraryA` / `LoadLibraryExA`) watched for the
/// reader's wrapper.
pub(crate) fn init() {
    if crate::frontend_fit::exe_name() != "amplayer.exe" {
        return;
    }
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "LoadLibraryA", player_load as *const () as usize) } {
        ORIG_PLAYER_LOAD.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "LoadLibraryExA", player_load_ex as *const () as usize) } {
        ORIG_PLAYER_LOAD_EX.store(o, Ordering::Relaxed);
    }
    hook_wrapper();
}

/// The frontend's `LoadLibraryA`.
unsafe extern "system" fn player_load(name: *const u8) -> usize {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_PLAYER_LOAD.load(Ordering::Relaxed)) };
    let module = unsafe { original(name) };
    hook_wrapper();
    module
}

/// The frontend's `LoadLibraryExA`.
unsafe extern "system" fn player_load_ex(name: *const u8, file: usize, flags: u32) -> usize {
    let original: LoadLibraryExA = unsafe { std::mem::transmute(ORIG_PLAYER_LOAD_EX.load(Ordering::Relaxed)) };
    let module = unsafe { original(name, file, flags) };
    hook_wrapper();
    module
}

/// The data engine's `LoadLibraryA` (its delay-loaded `GVRStorageDevice.dll`, at its first card
/// call).
unsafe extern "system" fn plusde_load(name: *const u8) -> usize {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_PLUSDE_LOAD.load(Ordering::Relaxed)) };
    let module = unsafe { original(name) };
    hook_wrapper();
    module
}

/// The data engine once loaded (with the frontend's plug-ins): its loads watched; the reader's
/// wrapper once loaded: its `LoadLibraryA` hooked.
fn hook_wrapper() {
    if ORIG_PLUSDE_LOAD.load(Ordering::Relaxed) == 0 {
        let plusde = unsafe { GetModuleHandleA(c"PLUSDE.dll".as_ptr().cast()) } as usize;
        if plusde != 0 {
            match unsafe { iat::hook_module(plusde, "kernel32.dll", "LoadLibraryA", plusde_load as *const () as usize) } {
                Some(o) => ORIG_PLUSDE_LOAD.store(o, Ordering::Relaxed),
                None => {
                    // not hooked again
                    ORIG_PLUSDE_LOAD.store(usize::MAX, Ordering::Relaxed);
                    log!("smartcard: PLUSDE.dll without a LoadLibraryA import");
                }
            }
        }
    }
    if ORIG_WRAPPER_LOAD.load(Ordering::Relaxed) != 0 {
        return;
    }
    let wrapper = unsafe { GetModuleHandleA(c"GVRStorageDevice.dll".as_ptr().cast()) } as usize;
    if wrapper == 0 {
        return;
    }
    match unsafe { iat::hook_module(wrapper, "kernel32.dll", "LoadLibraryA", wrapper_load as *const () as usize) } {
        Some(o) => {
            ORIG_WRAPPER_LOAD.store(o, Ordering::Relaxed);
            log!("smartcard: GVRStorageDevice.dll hooked");
        }
        None => {
            ORIG_WRAPPER_LOAD.store(usize::MAX, Ordering::Relaxed);
            log!("smartcard: GVRStorageDevice.dll without a LoadLibraryA import");
        }
    }
}

/// The wrapper's `LoadLibraryA` (the reader's module): this DLL for `GVRSCR28.dll`.
unsafe extern "system" fn wrapper_load(name: *const u8) -> usize {
    if !name.is_null() {
        let n = unsafe { CStr::from_ptr(name.cast()) }.to_string_lossy().to_ascii_lowercase();
        if n.rsplit(['/', '\\']).next() == Some("gvrscr28.dll") {
            log!("smartcard: LoadLibraryA({n:?}): the payload's reader");
            // a reference the wrapper's FreeLibrary drops (after each presence check)
            let mut module = std::ptr::null_mut();
            unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, wrapper_load as *const () as *const u16, &mut module) };
            return module as usize;
        }
    }
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_WRAPPER_LOAD.load(Ordering::Relaxed)) };
    unsafe { original(name) }
}

fn card_id() -> i64 {
    std::env::var("WAL_GLOBALVR_CARD_ID").ok().and_then(|v| v.trim().parse().ok()).filter(|&id| id > 0).unwrap_or(DEFAULT_ID)
}

/// The `card` control's presses: insert, then remove. The frontend asks the card out after
/// each read of its id (its login): out by itself `EJECT_AFTER_READ` after that read.
fn watch_card() {
    let mut was = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(16));
        let pressed = wal_payload_common::input(0).pressed(wal_protocol::button::CARD);
        if pressed && !was {
            let inserted = !INSERTED.fetch_xor(true, Ordering::Relaxed);
            *READ_AT.lock().unwrap() = None;
            log!("smartcard: card {}", if inserted { "inserted" } else { "removed" });
        }
        was = pressed;
        let mut read_at = READ_AT.lock().unwrap();
        if read_at.is_some_and(|t| t.elapsed() >= EJECT_AFTER_READ) {
            *read_at = None;
            INSERTED.store(false, Ordering::Relaxed);
            log!("smartcard: card removed after its read");
        }
    }
}

fn inserted() -> bool {
    INSERTED.load(Ordering::Relaxed)
}

/// The reader object: its methods' table, the error text the wrapper copies.
#[repr(C)]
struct Device {
    vtable: &'static Vtable,
    last_error: [u8; 1024],
}

type This = *mut Device;

/// The reader's methods, in the wrapper's order.
#[repr(C)]
struct Vtable {
    initialize: unsafe extern "thiscall" fn(This, i64) -> i32,
    shutdown: unsafe extern "thiscall" fn(This) -> i32,
    is_present: unsafe extern "thiscall" fn(This) -> bool,
    get_status: unsafe extern "thiscall" fn(This, *mut i32) -> i32,
    connect: unsafe extern "thiscall" fn(This) -> i32,
    disconnect: unsafe extern "thiscall" fn(This) -> i32,
    get_id: unsafe extern "thiscall" fn(This, *mut i64) -> i32,
    get_type: unsafe extern "thiscall" fn(This) -> u8,
    get_size: unsafe extern "thiscall" fn(This) -> i32,
    get_manufacturer_info: unsafe extern "thiscall" fn(This, *mut u8) -> i32,
    format: unsafe extern "thiscall" fn(This, *mut u8, i32, i64) -> i32,
    set_access_indicator: unsafe extern "thiscall" fn(This, bool) -> bool,
    read: unsafe extern "thiscall" fn(This, i32, i32, *mut u8) -> i32,
    write: unsafe extern "thiscall" fn(This, i32, i32, *const u8) -> i32,
    read_mag_stripe: unsafe extern "thiscall" fn(This, *mut u8) -> i32,
    swipe_start: unsafe extern "thiscall" fn(This) -> i32,
    swipe_detected: unsafe extern "thiscall" fn(This, *mut c_void) -> i32,
    swipe_stop: unsafe extern "thiscall" fn(This) -> i32,
}

static VTABLE: Vtable = Vtable {
    initialize,
    shutdown,
    is_present,
    get_status,
    connect,
    disconnect,
    get_id,
    get_type,
    get_size,
    get_manufacturer_info,
    format,
    set_access_indicator,
    read,
    write,
    read_mag_stripe,
    swipe_start,
    swipe_detected,
    swipe_stop,
};

/// `GVRStorageDeviceImp *CreateGVRStorageDeviceImp()`.
#[unsafe(no_mangle)]
pub extern "C" fn smartcard_create() -> *mut c_void {
    static THREAD: std::sync::Once = std::sync::Once::new();
    THREAD.call_once(|| {
        std::thread::spawn(watch_card);
    });
    log!("smartcard: reader created, card id {}", card_id());
    Box::into_raw(Box::new(Device { vtable: &VTABLE, last_error: [0; 1024] })).cast()
}

/// `void ReleaseGVRStorageDeviceImp(GVRStorageDeviceImp *)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn smartcard_release(device: *mut c_void) {
    if !device.is_null() {
        drop(unsafe { Box::from_raw(device.cast::<Device>()) });
    }
}

unsafe extern "thiscall" fn initialize(_this: This, _signature: i64) -> i32 {
    let mut memory = MEMORY.lock().unwrap();
    if memory.is_empty() {
        memory.resize(CARD_SIZE, 0);
    }
    OK
}

unsafe extern "thiscall" fn shutdown(_this: This) -> i32 {
    OK
}

unsafe extern "thiscall" fn is_present(_this: This) -> bool {
    true
}

unsafe extern "thiscall" fn get_status(_this: This, status: *mut i32) -> i32 {
    if let Some(status) = unsafe { status.as_mut() } {
        *status = if inserted() { STATUS_INSERTED } else { STATUS_EMPTY };
    }
    OK
}

unsafe extern "thiscall" fn connect(_this: This) -> i32 {
    if inserted() { OK } else { NOT_CONNECTED }
}

unsafe extern "thiscall" fn disconnect(_this: This) -> i32 {
    OK
}

unsafe extern "thiscall" fn get_id(_this: This, id: *mut i64) -> i32 {
    if !inserted() {
        return NOT_CONNECTED;
    }
    if let Some(id) = unsafe { id.as_mut() } {
        *id = card_id();
    }
    READ_AT.lock().unwrap().get_or_insert_with(Instant::now);
    OK
}

/// A player card.
unsafe extern "thiscall" fn get_type(_this: This) -> u8 {
    0
}

unsafe extern "thiscall" fn get_size(_this: This) -> i32 {
    if inserted() { CARD_SIZE as i32 } else { -1 }
}

unsafe extern "thiscall" fn get_manufacturer_info(_this: This, buffer: *mut u8) -> i32 {
    if !buffer.is_null() {
        let text = b"GVR\0";
        unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), buffer, text.len()) };
    }
    OK
}

unsafe extern "thiscall" fn format(_this: This, _buffer: *mut u8, _is_player: i32, _id: i64) -> i32 {
    MEMORY.lock().unwrap().fill(0);
    OK
}

unsafe extern "thiscall" fn set_access_indicator(_this: This, _on: bool) -> bool {
    true
}

/// The card memory's `[index, index + size)`, if inside it.
fn range(index: i32, size: i32) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(index).ok()?;
    let end = start.checked_add(usize::try_from(size).ok()?)?;
    (end <= CARD_SIZE).then_some(start..end)
}

unsafe extern "thiscall" fn read(_this: This, index: i32, size: i32, buffer: *mut u8) -> i32 {
    let (Some(r), false) = (range(index, size), buffer.is_null()) else { return ERROR };
    let memory = MEMORY.lock().unwrap();
    let Some(data) = memory.get(r) else { return ERROR };
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer, data.len()) };
    OK
}

unsafe extern "thiscall" fn write(_this: This, index: i32, size: i32, buffer: *const u8) -> i32 {
    let (Some(r), false) = (range(index, size), buffer.is_null()) else { return ERROR };
    let mut memory = MEMORY.lock().unwrap();
    let Some(data) = memory.get_mut(r) else { return ERROR };
    unsafe { std::ptr::copy_nonoverlapping(buffer, data.as_mut_ptr(), data.len()) };
    OK
}

/// No magnetic stripe reader.
unsafe extern "thiscall" fn read_mag_stripe(_this: This, _buffer: *mut u8) -> i32 {
    ERROR
}

unsafe extern "thiscall" fn swipe_start(_this: This) -> i32 {
    OK
}

unsafe extern "thiscall" fn swipe_detected(_this: This, _swipe: *mut c_void) -> i32 {
    0
}

unsafe extern "thiscall" fn swipe_stop(_this: This) -> i32 {
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_memory_range() {
        assert_eq!(range(0, 0x2000), Some(0..0x2000));
        assert_eq!(range(0x1ff0, 0x10), Some(0x1ff0..0x2000));
        assert_eq!(range(0x1ff0, 0x11), None);
        assert_eq!(range(-1, 4), None);
        assert_eq!(range(4, -1), None);
    }
}
