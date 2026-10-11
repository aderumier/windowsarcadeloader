//! `WAL_NESYS_TRACE=1`: logs the game's messages on the NESYS pipe (`\\.\pipe\nesys_games`), both
//! ways, whatever serves it (our `nesys.rs` or another service): finding what a game expects.
//! Hooks of the game's CreateFileA/W, WriteFile, ReadFile and GetOverlappedResult; overlapped
//! reads are logged when they complete.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

const COUNT: usize = 5;
static ORIG: [AtomicUsize; COUNT] = [const { AtomicUsize::new(0) }; COUNT];
static PIPE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Pending overlapped reads: (OVERLAPPED, buffer).
static PENDING: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn orig<F: Copy>(idx: usize) -> F {
    let p = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

pub(crate) fn init() {
    if std::env::var("WAL_NESYS_TRACE").map_or(true, |v| v != "1") {
        return;
    }
    let hooks: [(&str, usize); COUNT] = [
        ("CreateFileA", create_a as *const () as usize),
        ("CreateFileW", create_w as *const () as usize),
        ("WriteFile", write as *const () as usize),
        ("ReadFile", read as *const () as usize),
        ("GetOverlappedResult", overlapped as *const () as usize),
    ];
    for (idx, (name, f)) in hooks.into_iter().enumerate() {
        if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            ORIG[idx].store(o, Ordering::Relaxed);
        }
    }
    log!("nesystrace: enabled");
}

/// Logs one message: command and length (first 8 bytes), the data in hex (at most 256 bytes).
fn dump(dir: &str, data: &[u8]) {
    if data.len() >= 8 {
        let cmd = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let len = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let body: String = data[8..].iter().take(256).map(|b| format!("{b:02x}")).collect();
        log!("nesystrace: {dir} {cmd:#x} len {len} ({} bytes) {body}", data.len());
    } else {
        log!("nesystrace: {dir} {} bytes {:02x?}", data.len(), data);
    }
}

fn watch(name: &str, handle: usize) {
    if name.to_ascii_lowercase().contains("nesys") {
        log!("nesystrace: {name} opened: {handle:#x}");
        PIPE.store(handle, Ordering::Relaxed);
    }
}

unsafe extern "system" fn create_a(name: *const u8, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    let h = unsafe { orig::<unsafe extern "system" fn(*const u8, u32, u32, usize, u32, u32, usize) -> usize>(0)(name, access, share, sa, disp, flags, template) };
    if !name.is_null() {
        watch(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy(), h);
    }
    h
}

unsafe extern "system" fn create_w(name: *const u16, access: u32, share: u32, sa: usize, disp: u32, flags: u32, template: usize) -> usize {
    let h = unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, usize, u32, u32, usize) -> usize>(1)(name, access, share, sa, disp, flags, template) };
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        watch(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }), h);
    }
    h
}

unsafe extern "system" fn write(h: usize, buf: *const u8, len: u32, written: *mut u32, ov: usize) -> i32 {
    if h == PIPE.load(Ordering::Relaxed) && !buf.is_null() {
        dump("game ->", unsafe { std::slice::from_raw_parts(buf, len as usize) });
    }
    unsafe { orig::<unsafe extern "system" fn(usize, *const u8, u32, *mut u32, usize) -> i32>(2)(h, buf, len, written, ov) }
}

unsafe extern "system" fn read(h: usize, buf: *mut u8, len: u32, done: *mut u32, ov: usize) -> i32 {
    let r = unsafe { orig::<unsafe extern "system" fn(usize, *mut u8, u32, *mut u32, usize) -> i32>(3)(h, buf, len, done, ov) };
    if h == PIPE.load(Ordering::Relaxed) && !buf.is_null() {
        if r != 0 && !done.is_null() {
            dump("<- service", unsafe { std::slice::from_raw_parts(buf, *done as usize) });
        } else if ov != 0 {
            PENDING.lock().unwrap().push((ov, buf as usize));
        }
    }
    r
}

unsafe extern "system" fn overlapped(h: usize, ov: usize, done: *mut u32, wait: i32) -> i32 {
    let r = unsafe { orig::<unsafe extern "system" fn(usize, usize, *mut u32, i32) -> i32>(4)(h, ov, done, wait) };
    if r != 0 && h == PIPE.load(Ordering::Relaxed) && !done.is_null() {
        let mut pending = PENDING.lock().unwrap();
        if let Some(i) = pending.iter().position(|p| p.0 == ov) {
            let (_, buf) = pending.remove(i);
            dump("<- service", unsafe { std::slice::from_raw_parts(buf as *const u8, *done as usize) });
        }
    }
    r
}
