//! `WAL_ALLS_STAT_BYPASS=<file>,...`: the C runtime's `_stat64i32` (msvcr110) succeeds for paths
//! naming these files without reading them (segatools' idzhook `crt.c`, public domain): Initial
//! D ARCADE STAGE Zero and its amdaemon check the programs' sizes, the unpacked programs' differ.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

static NAMES: OnceLock<Vec<String>> = OnceLock::new();
static REAL: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn stat64i32(path: *const c_char, buffer: *mut c_void) -> i32 {
    if !path.is_null() {
        let p = unsafe { CStr::from_ptr(path) }.to_string_lossy();
        if NAMES.get().is_some_and(|n| n.iter().any(|n| p.contains(n.as_str()))) {
            log!("alls: _stat64i32({p}) passed");
            return 0;
        }
    }
    let real: unsafe extern "C" fn(*const c_char, *mut c_void) -> i32 = unsafe { std::mem::transmute(REAL.load(Ordering::Relaxed)) };
    unsafe { real(path, buffer) }
}

pub(crate) fn init() {
    let Ok(v) = std::env::var("WAL_ALLS_STAT_BYPASS") else { return };
    let _ = NAMES.set(v.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect());
    match unsafe { iat::hook("msvcr110.dll", "_stat64i32", stat64i32 as *const () as usize) } {
        Some(o) => REAL.store(o, Ordering::Relaxed),
        None => log!("alls: _stat64i32 not imported"),
    }
}
