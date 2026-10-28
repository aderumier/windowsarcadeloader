//! The Anark player's logger (`AKLog2`, a COM server of Anark's developer tools, never shipped
//! with the cabinets): the player's messages, its script errors ("Script Error: <behavior>(<line>)")
//! and its scripts' `output()` lines, in the payload log. For debugging the cabinet frontend: the
//! payload answers the logger's class (`DllGetClassObject`) once a registry layer registers it as
//! its in-process server (CLSID `{DE411D6F-2BDA-444C-AC87-9171BAFD9E99}`, `InprocServer32` = the
//! payload installed next to the player, `GvrIO.dll`).
//!
//! The player calls (interface `{18725949-89C4-460C-A092-75520BFFB542}`): +0x1C (a, b), +0x24 (),
//! +0x28 (record): a 0x42C-byte record, its UTF-16 message at +0x8 (a format, its arguments a
//! `va_list` at +0x210: the behavior's name and the line of a script error), its source file at
//! +0x21C. The rest answer S_OK.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering};

use wal_payload_common::log;

#[repr(C)]
#[derive(PartialEq, Eq)]
pub struct Guid(u32, u16, u16, [u8; 8]);

const CLSID_AKLOG2: Guid = Guid(0xDE411D6F, 0x2BDA, 0x444C, [0xAC, 0x87, 0x91, 0x71, 0xBA, 0xFD, 0x9E, 0x99]);
const IID_AKLOG2: Guid = Guid(0x18725949, 0x89C4, 0x460C, [0xA0, 0x92, 0x75, 0x52, 0x0B, 0xFF, 0xB5, 0x42]);
const IID_UNKNOWN: Guid = Guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_CLASS_FACTORY: Guid = Guid(1, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const S_OK: i32 = 0;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: i32 = 0x8004_0111u32 as i32;

type P = *mut c_void;

#[repr(C)]
struct Object {
    vtable: &'static Vtable,
}

/// A COM method table.
#[repr(C)]
struct Vtable([*const (); 16]);

// constant method tables
unsafe impl Sync for Vtable {}

static LOGGER: Object = Object { vtable: &LOGGER_VTABLE };
static FACTORY: Object = Object { vtable: &FACTORY_VTABLE };
static REFS: AtomicI32 = AtomicI32::new(1);

static LOGGER_VTABLE: Vtable = Vtable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    ok as *const (),
    ok as *const (),
    ok as *const (),
    ok as *const (),
    two_args as *const (),
    ok as *const (),
    ok as *const (),
    record as *const (),
    ok as *const (),
    ok as *const (),
    ok as *const (),
    ok as *const (),
    ok as *const (),
]);

static FACTORY_VTABLE: Vtable = Vtable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    create as *const (),
    lock as *const (),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
    std::ptr::null(),
]);

unsafe extern "system" fn query(this: P, iid: *const Guid, out: *mut P) -> i32 {
    let iid = unsafe { &*iid };
    if *iid == IID_UNKNOWN || *iid == IID_AKLOG2 || *iid == IID_CLASS_FACTORY {
        unsafe { *out = this };
        return S_OK;
    }
    unsafe { *out = std::ptr::null_mut() };
    E_NOINTERFACE
}

unsafe extern "system" fn add_ref(_this: P) -> u32 {
    REFS.fetch_add(1, Ordering::Relaxed) as u32 + 1
}

unsafe extern "system" fn release(_this: P) -> u32 {
    // static objects: never freed
    (REFS.fetch_sub(1, Ordering::Relaxed) - 1).max(1) as u32
}

unsafe extern "system" fn ok(_this: P) -> i32 {
    S_OK
}

unsafe extern "system" fn lock(_this: P, _lock: i32) -> i32 {
    S_OK
}

unsafe extern "system" fn create(_this: P, outer: P, iid: *const Guid, out: *mut P) -> i32 {
    if !outer.is_null() {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    log!("aklog: the player's logger created");
    unsafe { query(&LOGGER as *const Object as P, iid, out) }
}

unsafe extern "system" fn two_args(_this: P, a: usize, b: usize) -> i32 {
    log!("aklog: {} {}", text(a), text(b));
    S_OK
}

fn readable(at: usize, len: usize) -> bool {
    at >= 0x10000 && wal_payload_common::crash::readable(at, len)
}

/// A UTF-16 or ASCII string at `at`, else the number.
fn text(at: usize) -> String {
    if !readable(at, 4) {
        return format!("{at:#x}");
    }
    let wide = unsafe { std::slice::from_raw_parts(at as *const u8, 4) };
    if wide[1] == 0 && wide[3] == 0 {
        wstring(at)
    } else {
        (0..300)
            .map_while(|i| readable(at + i, 1).then(|| unsafe { *((at + i) as *const u8) }))
            .take_while(|&b| b != 0)
            .map(|b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect()
    }
}

fn wstring(at: usize) -> String {
    let units: Vec<u16> =
        (0..600).map_while(|i| readable(at + i * 2, 2).then(|| unsafe { *((at + i * 2) as *const u16) })).take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&units)
}

/// A message record: its text, its format's arguments (`%s` / `%ls` / `%d` / `%ld`) taken from
/// the `va_list` at +0x210.
unsafe extern "system" fn record(_this: P, rec: usize) -> i32 {
    if !readable(rec, 0x42c) {
        return S_OK;
    }
    let format = wstring(rec + 8);
    let mut args = unsafe { *((rec + 0x210) as *const usize) };
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' || !readable(args, 4) {
            out.push(c);
            continue;
        }
        let mut spec = String::new();
        while let Some(&n) = chars.peek() {
            chars.next();
            spec.push(n);
            if n.is_ascii_alphabetic() && n != 'l' && n != 'h' {
                break;
            }
        }
        let value = unsafe { *(args as *const usize) };
        args += 4;
        if spec.ends_with('s') {
            out.push_str(&text(value));
        } else {
            out.push_str(&(value as i32).to_string());
        }
    }
    let source = wstring(rec + 0x21c);
    log!("aklog: {}{}", out.trim_end(), if source.is_empty() { String::new() } else { format!(" [{source}]") });
    S_OK
}

/// The logger's class object.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(clsid: *const Guid, iid: *const Guid, out: *mut P) -> i32 {
    if unsafe { &*clsid } != &CLSID_AKLOG2 {
        unsafe { *out = std::ptr::null_mut() };
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    unsafe { query(&FACTORY as *const Object as P, iid, out) }
}

/// Never unloaded (its objects are static).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllCanUnloadNow() -> i32 {
    1
}
