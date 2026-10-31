//! `WAL_INI_KEYS=<key>=<value>;...`: settings the game reads from its ini files
//! (`GetPrivateProfileStringA`, any file and section) answered with these values, whatever the
//! file says or the game wrote there. Justice League: Heroes United routes every joystick to the
//! player it is detecting until its operator menu assigned them (`UseJoystickAutoDetect`,
//! `Joystick<n>SoftwareMap` in its JLH.ini).

use std::sync::OnceLock;

use crate::{iat, log};

static KEYS: OnceLock<Vec<(String, String)>> = OnceLock::new();
static ORIGINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

type GetString = unsafe extern "system" fn(*const u8, *const u8, *const u8, *mut u8, u32, *const u8) -> u32;

/// `GetPrivateProfileStringA(section, key, default, out, size, file)`
unsafe extern "system" fn get_string(section: *const u8, key: *const u8, default: *const u8, out: *mut u8, size: u32, file: *const u8) -> u32 {
    if !key.is_null() && !out.is_null() && size > 0 {
        let name = unsafe { std::ffi::CStr::from_ptr(key.cast()) }.to_string_lossy();
        if let Some((_, value)) = KEYS.get().and_then(|keys| keys.iter().find(|(k, _)| k.eq_ignore_ascii_case(&name))) {
            let n = value.len().min(size as usize - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(value.as_ptr(), out, n);
                *out.add(n) = 0;
            }
            return n as u32;
        }
    }
    let original: GetString = unsafe { std::mem::transmute(ORIGINAL.load(std::sync::atomic::Ordering::Relaxed)) };
    unsafe { original(section, key, default, out, size, file) }
}

/// The `key=value` pairs of a `WAL_INI_KEYS` value.
fn parse(spec: &str) -> Vec<(String, String)> {
    spec.split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

pub fn init() {
    let Ok(spec) = std::env::var("WAL_INI_KEYS") else { return };
    let keys = parse(&spec);
    if keys.is_empty() {
        return;
    }
    log!("ini: {} keys answered ({spec})", keys.len());
    let _ = KEYS.set(keys);
    let ours = get_string as *const () as usize;
    for module in crate::workarea::game_modules(iat::game_module()) {
        if let Some(o) = unsafe { iat::hook_module(module, "kernel32.dll", "GetPrivateProfileStringA", ours) }.filter(|&o| o != ours) {
            ORIGINAL.store(o, std::sync::atomic::Ordering::Relaxed);
        }
    }
    if ORIGINAL.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        log!("ini: the game does not import GetPrivateProfileStringA");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs() {
        assert_eq!(
            parse("UseJoystickAutoDetect=false; Joystick1SoftwareMap = 1;bad;"),
            vec![("UseJoystickAutoDetect".into(), "false".into()), ("Joystick1SoftwareMap".into(), "1".into())]
        );
    }
}
