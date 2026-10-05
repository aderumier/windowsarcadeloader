//! `WAL_ANSI_CODEPAGE=<cp>`: the game's own `MultiByteToWideChar` / `WideCharToMultiByte` calls
//! on the ANSI code page (CP_ACP, CP_THREAD_ACP) use `<cp>` instead (932 = Shift-JIS).
//! Japanese games convert Shift-JIS file names themselves: Raiden IV's movie
//! "raiden4_ｃ50.m1v" was not found under a western code page (white intro).

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::{iat, log};

const CP_ACP: u32 = 0;
const CP_THREAD_ACP: u32 = 3;

static CODEPAGE: AtomicU32 = AtomicU32::new(0);
static ORIG_MB2WC: AtomicUsize = AtomicUsize::new(0);
static ORIG_WC2MB: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    let Some(cp) = std::env::var("WAL_ANSI_CODEPAGE").ok().and_then(|v| v.trim().parse::<u32>().ok()) else {
        return;
    };
    CODEPAGE.store(cp, Ordering::Relaxed);
    unsafe {
        if let Some(o) = iat::hook("kernel32.dll", "MultiByteToWideChar", mb2wc as *const () as usize) {
            ORIG_MB2WC.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook("kernel32.dll", "WideCharToMultiByte", wc2mb as *const () as usize) {
            ORIG_WC2MB.store(o, Ordering::Relaxed);
        }
    }
    log!("codepage: game ANSI conversions use code page {cp}");
}

fn codepage(cp: u32) -> u32 {
    if cp == CP_ACP || cp == CP_THREAD_ACP { CODEPAGE.load(Ordering::Relaxed) } else { cp }
}

unsafe extern "system" fn mb2wc(cp: u32, flags: u32, src: *const u8, src_len: i32, dst: *mut u16, dst_len: i32) -> i32 {
    let orig: unsafe extern "system" fn(u32, u32, *const u8, i32, *mut u16, i32) -> i32 =
        unsafe { std::mem::transmute(ORIG_MB2WC.load(Ordering::Relaxed)) };
    unsafe { orig(codepage(cp), flags, src, src_len, dst, dst_len) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn wc2mb(
    cp: u32,
    flags: u32,
    src: *const u16,
    src_len: i32,
    dst: *mut u8,
    dst_len: i32,
    default_char: *const u8,
    used_default: *mut i32,
) -> i32 {
    let orig: unsafe extern "system" fn(u32, u32, *const u16, i32, *mut u8, i32, *const u8, *mut i32) -> i32 =
        unsafe { std::mem::transmute(ORIG_WC2MB.load(Ordering::Relaxed)) };
    unsafe { orig(codepage(cp), flags, src, src_len, dst, dst_len, default_char, used_default) }
}
