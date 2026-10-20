//! The Shell's settings the game reads itself (`GetPrivateProfileIntA` / `StringA`: Showdown
//! reads `ShellData.ini`, the Shell's operator settings), overridden by the profile:
//!
//! * `WAL_SEGASHELL_INI=<section>/<key>=<value>,...`: values (a value the dump's file holds
//!   that the game refuses, e.g. Showdown's `Audio/AttractMusic`, 0-3, written as a volume by
//!   later Shells)
//! * `WAL_SEGASHELL_FREEPLAY=0|1`: `Credit/Freeplay`

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

type GetPrivateProfileIntA = unsafe extern "system" fn(*const u8, *const u8, i32, *const u8) -> u32;
type GetPrivateProfileStringA = unsafe extern "system" fn(*const u8, *const u8, *const u8, *mut u8, u32, *const u8) -> u32;

static ORIG_INT: AtomicUsize = AtomicUsize::new(0);
static ORIG_STRING: AtomicUsize = AtomicUsize::new(0);

/// (section, key, value), from the profile.
fn overrides() -> &'static [(String, String, String)] {
    static LIST: OnceLock<Vec<(String, String, String)>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut list: Vec<(String, String, String)> = std::env::var("WAL_SEGASHELL_INI")
            .unwrap_or_default()
            .split(',')
            .filter_map(|item| {
                let (name, value) = item.split_once('=')?;
                let (section, key) = name.trim().split_once('/')?;
                Some((section.trim().to_string(), key.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        if let Ok(v) = std::env::var("WAL_SEGASHELL_FREEPLAY").map(|v| v.trim().to_string()) {
            if !v.is_empty() {
                list.push(("Credit".into(), "Freeplay".into(), v));
            }
        }
        list
    })
}

fn c_str(p: *const u8) -> String {
    if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() }
}

/// The profile's value of `[section] key`.
fn value(section: *const u8, key: *const u8) -> Option<&'static str> {
    if section.is_null() || key.is_null() {
        return None;
    }
    let (s, k) = (c_str(section), c_str(key));
    let (_, _, v) = overrides().iter().find(|(os, ok, _)| os.eq_ignore_ascii_case(&s) && ok.eq_ignore_ascii_case(&k))?;
    log!("settings: [{s}] {k} = {v}");
    Some(v)
}

unsafe extern "system" fn get_int(section: *const u8, key: *const u8, default: i32, file: *const u8) -> u32 {
    if let Some(v) = value(section, key).and_then(|v| v.parse::<i32>().ok()) {
        return v as u32;
    }
    let f: GetPrivateProfileIntA = unsafe { std::mem::transmute(ORIG_INT.load(Ordering::Relaxed)) };
    unsafe { f(section, key, default, file) }
}

unsafe extern "system" fn get_string(section: *const u8, key: *const u8, default: *const u8, out: *mut u8, size: u32, file: *const u8) -> u32 {
    if let Some(v) = value(section, key).filter(|_| !out.is_null() && size > 0) {
        let n = v.len().min(size as usize - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(v.as_ptr(), out, n);
            *out.add(n) = 0;
        }
        return n as u32;
    }
    let f: GetPrivateProfileStringA = unsafe { std::mem::transmute(ORIG_STRING.load(Ordering::Relaxed)) };
    unsafe { f(section, key, default, out, size, file) }
}

pub(crate) fn init() {
    if overrides().is_empty() {
        return;
    }
    let hooks = [("GetPrivateProfileIntA", get_int as *const () as usize, &ORIG_INT), ("GetPrivateProfileStringA", get_string as *const () as usize, &ORIG_STRING)];
    for (name, f, orig) in hooks {
        if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
}
