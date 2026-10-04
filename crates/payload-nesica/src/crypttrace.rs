//! `WAL_TRACE_CRYPT=1`: logs the game's own CryptoAPI calls (debugging content decryption).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::GetLastError;

const COUNT: usize = 5;
static ORIG: [AtomicUsize; COUNT] = [const { AtomicUsize::new(0) }; COUNT];
static DECRYPTS: AtomicUsize = AtomicUsize::new(0);

fn orig<F: Copy>(idx: usize) -> F {
    let p = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

unsafe fn bytes<'a>(p: *const u8, len: u32) -> &'a [u8] {
    if p.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(p, len as usize) } }
}

unsafe fn c_str(p: *const u8) -> String {
    if p.is_null() { "(null)".into() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() }
}

pub(crate) fn init() {
    if std::env::var("WAL_TRACE_CRYPT").map_or(true, |v| v != "1") {
        return;
    }
    let hooks: [(&str, usize); COUNT] = [
        ("CryptAcquireContextA", acquire as *const () as usize),
        ("CryptImportKey", import as *const () as usize),
        ("CryptDecrypt", decrypt as *const () as usize),
        ("CryptExportKey", export as *const () as usize),
        ("CryptGenKey", gen_key as *const () as usize),
    ];
    for (idx, (name, f)) in hooks.into_iter().enumerate() {
        if let Some(o) = unsafe { iat::hook("advapi32.dll", name, f) } {
            ORIG[idx].store(o, Ordering::Relaxed);
        }
    }
    log!("crypttrace: enabled");
}

unsafe extern "system" fn acquire(prov: *mut usize, container: *const u8, provider: *const u8, ty: u32, flags: u32) -> i32 {
    let r = unsafe { orig::<unsafe extern "system" fn(*mut usize, *const u8, *const u8, u32, u32) -> i32>(0)(prov, container, provider, ty, flags) };
    let err = unsafe { GetLastError() };
    log!("crypttrace: AcquireContext({}, {}, type {ty}, flags {flags:#x}) -> {r} err {err:#x}", unsafe { c_str(container) }, unsafe { c_str(provider) });
    r
}

unsafe extern "system" fn import(prov: usize, data: *const u8, len: u32, pubkey: usize, flags: u32, key: *mut usize) -> i32 {
    let r = unsafe { orig::<unsafe extern "system" fn(usize, *const u8, u32, usize, u32, *mut usize) -> i32>(1)(prov, data, len, pubkey, flags, key) };
    let err = unsafe { GetLastError() };
    let blob = unsafe { bytes(data, len.min(24)) };
    log!("crypttrace: ImportKey(len {len}, with {pubkey:#x}, flags {flags:#x}) head {} -> {r} key {:#x} err {err:#x}", hex(blob), if key.is_null() { 0 } else { unsafe { *key } });
    r
}

unsafe extern "system" fn decrypt(key: usize, hash: usize, fin: i32, flags: u32, data: *mut u8, len: *mut u32) -> i32 {
    let before = if len.is_null() { 0 } else { unsafe { *len } };
    let head_in = hex(unsafe { bytes(data, before.min(16)) });
    let r = unsafe { orig::<unsafe extern "system" fn(usize, usize, i32, u32, *mut u8, *mut u32) -> i32>(2)(key, hash, fin, flags, data, len) };
    let err = unsafe { GetLastError() };
    let after = if len.is_null() { 0 } else { unsafe { *len } };
    if r == 0 || DECRYPTS.fetch_add(1, Ordering::Relaxed) < 20 {
        log!(
            "crypttrace: Decrypt(key {key:#x}, hash {hash:#x}, final {fin}, flags {flags:#x}, len {before}) in {head_in} -> {r} len {after} out {} err {err:#x}",
            hex(unsafe { bytes(data, after.min(16)) })
        );
    }
    r
}

unsafe extern "system" fn export(key: usize, exp: usize, ty: u32, flags: u32, data: *mut u8, len: *mut u32) -> i32 {
    let r = unsafe { orig::<unsafe extern "system" fn(usize, usize, u32, u32, *mut u8, *mut u32) -> i32>(3)(key, exp, ty, flags, data, len) };
    log!("crypttrace: ExportKey(key {key:#x}, with {exp:#x}, type {ty}, flags {flags:#x}) -> {r} len {}", if len.is_null() { 0 } else { unsafe { *len } });
    r
}

unsafe extern "system" fn gen_key(prov: usize, alg: u32, flags: u32, key: *mut usize) -> i32 {
    let r = unsafe { orig::<unsafe extern "system" fn(usize, u32, u32, *mut usize) -> i32>(4)(prov, alg, flags, key) };
    log!("crypttrace: GenKey(alg {alg:#x}, flags {flags:#x}) -> {r} key {:#x}", if key.is_null() { 0 } else { unsafe { *key } });
    r
}
