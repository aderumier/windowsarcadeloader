//! `WAL_ALLS_INI_FILE=<file>:<key>=<value>,...`: a settings file of the game (read with its own
//! parser) opened as a copy with these keys' values replaced (`Key = Value` lines, any section;
//! the copy in `Y:\`), the dump's file unchanged. Initial D ARCADE STAGE Zero's `initialD.ini`:
//! the screen's size and full screen mode (the dump's says 2560x1440).
//! `{screen_width}` / `{screen_height}` in a value: the desktop's size.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

/// (file name, the copy's path)
static FILE: OnceLock<(String, CString)> = OnceLock::new();
static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);
static FOPEN: AtomicUsize = AtomicUsize::new(0);
static FOPEN_S: AtomicUsize = AtomicUsize::new(0);

/// The copy's path when `path` names the file.
fn redirect(path: *const c_char) -> Option<*const c_char> {
    if path.is_null() {
        return None;
    }
    let (name, copy) = FILE.get()?;
    let p = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    let file = p.rsplit(['\\', '/']).next().unwrap_or(&p);
    file.eq_ignore_ascii_case(name).then(|| {
        log!("alls: {p} read from {}", copy.to_string_lossy());
        copy.as_ptr()
    })
}

unsafe extern "system" fn create_file_a(path: *const c_char, access: u32, share: u32, sa: *const c_void, disposition: u32, flags: u32, template: *mut c_void) -> *mut c_void {
    let real: unsafe extern "system" fn(*const c_char, u32, u32, *const c_void, u32, u32, *mut c_void) -> *mut c_void = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    unsafe { real(redirect(path).unwrap_or(path), access, share, sa, disposition, flags, template) }
}

unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut c_void {
    let real: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void = unsafe { std::mem::transmute(FOPEN.load(Ordering::Relaxed)) };
    unsafe { real(redirect(path).unwrap_or(path), mode) }
}

unsafe extern "C" fn fopen_s(out: *mut *mut c_void, path: *const c_char, mode: *const c_char) -> i32 {
    let real: unsafe extern "C" fn(*mut *mut c_void, *const c_char, *const c_char) -> i32 = unsafe { std::mem::transmute(FOPEN_S.load(Ordering::Relaxed)) };
    unsafe { real(out, redirect(path).unwrap_or(path), mode) }
}

/// `text` with each `key = value` line of `keys` given the new value (its spacing kept).
fn edit(text: &str, keys: &[(String, String)]) -> String {
    text.split_inclusive('\n')
        .map(|line| {
            let Some((k, rest)) = line.split_once('=') else { return line.to_string() };
            let key = k.trim();
            if key.starts_with(';') {
                return line.to_string();
            }
            match keys.iter().find(|(n, _)| n.eq_ignore_ascii_case(key)) {
                Some((_, v)) => {
                    let end = if rest.ends_with("\r\n") { "\r\n" } else if rest.ends_with('\n') { "\n" } else { "" };
                    let space = if rest.starts_with(' ') { " " } else { "" };
                    format!("{k}={space}{v}{end}")
                }
                None => line.to_string(),
            }
        })
        .collect()
}

pub(crate) fn init() {
    let Ok(spec) = std::env::var("WAL_ALLS_INI_FILE") else { return };
    let Some((name, list)) = spec.split_once(':') else { return };
    let (w, h) = wal_payload_common::desktop::size().unwrap_or_else(wal_payload_common::desktop::screen_size);
    let keys: Vec<(String, String)> = list
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().replace("{screen_width}", &w.to_string()).replace("{screen_height}", &h.to_string())))
        .collect();
    let name = name.trim().to_string();
    let Ok(bytes) = std::fs::read(&name) else {
        log!("alls: {name} not found");
        return;
    };
    // the file's text is Shift-JIS: edited as Latin-1 (bytes kept)
    let text: String = bytes.iter().map(|&b| b as char).collect();
    let out: Vec<u8> = edit(&text, &keys).chars().map(|c| c as u32 as u8).collect();
    let copy = format!("Y:\\{name}");
    if let Err(e) = std::fs::write(&copy, out) {
        log!("alls: {copy}: {e}");
        return;
    }
    let _ = FILE.set((name.clone(), CString::new(copy).expect("path")));
    log!("alls: {name} read as a copy with {keys:?}");
    unsafe {
        if let Some(o) = iat::hook("kernel32.dll", "CreateFileA", create_file_a as *const () as usize) {
            CREATE_FILE_A.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("msvcr110.dll", "fopen", fopen as *const () as usize) {
            FOPEN.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("msvcr110.dll", "fopen_s", fopen_s as *const () as usize) {
            FOPEN_S.store(o, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn edits_keys() {
        let keys = vec![("Width".to_string(), "1920".to_string()), ("FullScreen".to_string(), "0".to_string())];
        let text = ";; Width = 1\r\nFullScreen = 1\r\nWidth  = 2560\r\nHeight = 1440\r\n";
        assert_eq!(super::edit(text, &keys), ";; Width = 1\r\nFullScreen = 0\r\nWidth  = 1920\r\nHeight = 1440\r\n");
    }
}
