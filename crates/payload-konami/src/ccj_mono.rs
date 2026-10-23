//! Chase Chase Jokers' managed settings, set through Mono's embedding API
//! (`mono-2.0-bdwgc.dll`'s exports): a static field of a game class.
//!
//! * `BootItemIO.processed`: the boot's I/O check has looked at the monitors. The cabinet has a
//!   1920x1080 main monitor and a 1280x1024 sub monitor; anything else stops the boot on a
//!   monitor error (5-1525) until Service is pressed. Set, the check only reports the I/O
//!   board's status.
//!
//! Called from the game's managed threads (an AIO call), never from `DllMain`.

use std::ffi::{CString, c_char, c_void};

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

type P = *mut c_void;

/// Mono's exports used here.
struct Mono {
    get_root_domain: unsafe extern "C" fn() -> P,
    thread_attach: unsafe extern "C" fn(P) -> P,
    image_loaded: unsafe extern "C" fn(*const c_char) -> P,
    class_from_name: unsafe extern "C" fn(P, *const c_char, *const c_char) -> P,
    class_get_field_from_name: unsafe extern "C" fn(P, *const c_char) -> P,
    class_vtable: unsafe extern "C" fn(P, P) -> P,
    field_static_set_value: unsafe extern "C" fn(P, P, P),
}

fn mono() -> Option<Mono> {
    let module = unsafe { GetModuleHandleW(windows_sys::w!("mono-2.0-bdwgc.dll")) };
    if module.is_null() {
        return None;
    }
    let get = |name: &str| {
        let name = CString::new(name).unwrap();
        unsafe { GetProcAddress(module, name.as_ptr() as *const u8) }.map(|f| f as usize)
    };
    unsafe {
        Some(Mono {
            get_root_domain: std::mem::transmute(get("mono_get_root_domain")?),
            thread_attach: std::mem::transmute(get("mono_thread_attach")?),
            image_loaded: std::mem::transmute(get("mono_image_loaded")?),
            class_from_name: std::mem::transmute(get("mono_class_from_name")?),
            class_get_field_from_name: std::mem::transmute(get("mono_class_get_field_from_name")?),
            class_vtable: std::mem::transmute(get("mono_class_vtable")?),
            field_static_set_value: std::mem::transmute(get("mono_field_static_set_value")?),
        })
    }
}

/// Sets the static `bool` field `class.field` of the assembly `assembly` (global namespace).
fn set_static_bool(assembly: &str, class: &str, field: &str, value: bool) -> bool {
    let Some(m) = mono() else {
        log!("ccj: Mono not found");
        return false;
    };
    let (assembly_c, class_c, field_c) = (CString::new(assembly).unwrap(), CString::new(class).unwrap(), CString::new(field).unwrap());
    unsafe {
        let domain = (m.get_root_domain)();
        if domain.is_null() {
            return false;
        }
        (m.thread_attach)(domain);
        let image = (m.image_loaded)(assembly_c.as_ptr());
        let klass = if image.is_null() { image } else { (m.class_from_name)(image, c"".as_ptr(), class_c.as_ptr()) };
        let f = if klass.is_null() { klass } else { (m.class_get_field_from_name)(klass, field_c.as_ptr()) };
        let vtable = if f.is_null() { f } else { (m.class_vtable)(domain, klass) };
        if vtable.is_null() {
            log!("ccj: {assembly} {class}.{field} not found");
            return false;
        }
        let mut v = value as u8;
        (m.field_static_set_value)(vtable, f, &mut v as *mut u8 as P);
    }
    true
}

/// The boot's monitor check passed (`BootItemIO.processed`).
pub fn skip_monitor_check() {
    if set_static_bool("ChaseGameScripts", "BootItemIO", "processed", true) {
        log!("ccj: boot monitor check skipped (one screen)");
    }
}
