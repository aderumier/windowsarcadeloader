//! The TsuMotion cabinet COM object (the motion seat, `tsumotion.exe`), emulated as an idle
//! seat that accepts everything.
//!
//! `-launchGame` / `-coinop` (not `-launchAttract`) create it at 0x4c8510:
//! `CoCreateInstance(CLSID {BAC8CFED-…}, CLSCTX_ALL, IID_IUnknown)` → `OleRun` → QI
//! `{BAC8CFEC-…}`, cached in the game global 0x9f74ac. A creation failure raises a COM error,
//! so the game cannot start a race without it. The arg counts come from the game's call sites
//! (stdcall: each slot pops exactly its args):
//!
//! * [9] GetLastError(out)       * [10] () per frame            * [11] (6 args) seat position
//! * [14] LoadEffect(name, id)   * [15] (3 args) play effect    * [19] (name, a, b)
//! * [20] () close               * [22] (index, out) limits
//!
//! Out values are 0 (no error, zero limits); every call returns S_OK.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::log;
use windows_sys::core::GUID;

type P = *mut c_void;
type HRESULT = i32;

const S_OK: HRESULT = 0;
const E_NOTIMPL: HRESULT = 0x8000_4001_u32 as i32;
const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;

/// {BAC8CFED-0530-11D6-9D8A-0001031DF57D}
pub const CLSID_TSUMOTION: GUID = GUID::from_u128(0xbac8cfed_0530_11d6_9d8a_0001031df57d);
/// {BAC8CFEC-0530-11D6-9D8A-0001031DF57D}
const IID_TSUMOTION: GUID = GUID::from_u128(0xbac8cfec_0530_11d6_9d8a_0001031df57d);
/// {00000000-0000-0000-C000-000000000046}
const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);

const N_SLOTS: usize = 32;
static VTABLE: [AtomicUsize; N_SLOTS] = [const { AtomicUsize::new(0) }; N_SLOTS];
static OBJECT_CELL: AtomicUsize = AtomicUsize::new(0);
static LOGGED: [AtomicU32; N_SLOTS] = [const { AtomicU32::new(0) }; N_SLOTS];

pub fn object_ptr() -> P {
    &OBJECT_CELL as *const AtomicUsize as P
}

fn log_once(slot: usize, what: &str) {
    if LOGGED[slot].swap(1, Ordering::Relaxed) == 0 {
        log!("tsumotion: m{slot} {what}");
    }
}

fn guid_eq(a: *const GUID, b: &GUID) -> bool {
    if a.is_null() {
        return false;
    }
    let a = unsafe { &*a };
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

unsafe extern "system" fn query_interface(this: P, riid: *const GUID, out: *mut P) -> HRESULT {
    let ok = guid_eq(riid, &IID_IUNKNOWN) || guid_eq(riid, &IID_TSUMOTION);
    if !out.is_null() {
        unsafe { *out = if ok { this } else { std::ptr::null_mut() } };
    }
    if ok { S_OK } else { E_NOINTERFACE }
}

unsafe extern "system" fn ref_count(_this: P) -> u32 {
    1 // static object
}

unsafe extern "system" fn not_impl(_this: P) -> HRESULT {
    E_NOTIMPL
}

/// [9] GetLastError(out): no error.
unsafe extern "system" fn m9(_this: P, out: *mut u32) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    log_once(9, "GetLastError");
    S_OK
}

unsafe extern "system" fn m10(_this: P) -> HRESULT {
    log_once(10, "Update");
    S_OK
}

unsafe extern "system" fn m11(_this: P, _a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32) -> HRESULT {
    log_once(11, "SetPosition");
    S_OK
}

/// [14] LoadEffect(name, id_out?): the game checks GetLastError afterwards.
unsafe extern "system" fn m14(_this: P, _name: P, _b: u32) -> HRESULT {
    log_once(14, "LoadEffect");
    S_OK
}

unsafe extern "system" fn m15(_this: P, _a: u32, _b: u32, _c: u32) -> HRESULT {
    log_once(15, "PlayEffect");
    S_OK
}

unsafe extern "system" fn m19(_this: P, _a: P, _b: u32, _c: u32) -> HRESULT {
    log_once(19, "Init");
    S_OK
}

unsafe extern "system" fn m20(_this: P) -> HRESULT {
    log_once(20, "Close");
    S_OK
}

/// [22] (index, out): seat limit `index` (0..5).
unsafe extern "system" fn m22(_this: P, _index: u32, out: *mut u32) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    log_once(22, "GetLimits");
    S_OK
}

pub fn init() {
    let known: [(usize, usize); 11] = [
        (0, query_interface as *const () as usize),
        (1, ref_count as *const () as usize),
        (2, ref_count as *const () as usize),
        (9, m9 as *const () as usize),
        (10, m10 as *const () as usize),
        (11, m11 as *const () as usize),
        (14, m14 as *const () as usize),
        (15, m15 as *const () as usize),
        (19, m19 as *const () as usize),
        (20, m20 as *const () as usize),
        (22, m22 as *const () as usize),
    ];
    for slot in VTABLE.iter() {
        slot.store(not_impl as *const () as usize, Ordering::Relaxed);
    }
    for (i, f) in known {
        VTABLE[i].store(f, Ordering::Relaxed);
    }
    OBJECT_CELL.store(VTABLE.as_ptr() as usize, Ordering::Relaxed);
}
