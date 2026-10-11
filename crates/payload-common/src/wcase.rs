//! `WAL_FAST_WCASE=1`: the game's own case-insensitive wide string calls (`_wcsicmp`,
//! `_wcsnicmp`, `towlower`, `towupper` of the C runtime) fold ASCII themselves. Wine's
//! `towlower` maps each character through `LCMapStringW`: Fighting EX Layer (Unreal Engine 4)
//! compares enough strings after a fight for its window to stop for ~5 s twice ("not
//! responding"). Other characters still go to the original `towlower` / `towupper`.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};

const DLLS: [&str; 2] = ["api-ms-win-crt-string-l1-1-0.dll", "ucrtbase.dll"];

static ORIG_LOWER: AtomicUsize = AtomicUsize::new(0);
static ORIG_UPPER: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    if std::env::var("WAL_FAST_WCASE").map_or(true, |v| v != "1") {
        return;
    }
    let hooks: [(&str, usize, Option<&AtomicUsize>); 4] = [
        ("towlower", towlower as *const () as usize, Some(&ORIG_LOWER)),
        ("towupper", towupper as *const () as usize, Some(&ORIG_UPPER)),
        ("_wcsicmp", wcsicmp as *const () as usize, None),
        ("_wcsnicmp", wcsnicmp as *const () as usize, None),
    ];
    let mut hooked = Vec::new();
    for (name, f, orig) in hooks {
        for dll in DLLS {
            if let Some(o) = unsafe { iat::hook(dll, name, f) } {
                if let Some(orig) = orig {
                    orig.store(o, Ordering::Relaxed);
                }
                hooked.push(name);
                break;
            }
        }
    }
    log!("wcase: fast {hooked:?}");
}

/// The C runtime's function, or the C locale's ASCII mapping when the game does not import it.
fn lower(c: u16) -> u16 {
    if c < 0x80 {
        return (c as u8).to_ascii_lowercase() as u16;
    }
    match ORIG_LOWER.load(Ordering::Relaxed) {
        0 => c,
        f => unsafe { std::mem::transmute::<usize, extern "C" fn(u16) -> u16>(f)(c) },
    }
}

extern "C" fn towlower(c: u16) -> u16 {
    lower(c)
}

extern "C" fn towupper(c: u16) -> u16 {
    if c < 0x80 {
        return (c as u8).to_ascii_uppercase() as u16;
    }
    match ORIG_UPPER.load(Ordering::Relaxed) {
        0 => c,
        f => unsafe { std::mem::transmute::<usize, extern "C" fn(u16) -> u16>(f)(c) },
    }
}

/// `_wcsnicmp`: compares at most `n` characters, lowercase.
unsafe fn compare(a: *const u16, b: *const u16, n: usize) -> i32 {
    for i in 0..n {
        let (ca, cb) = unsafe { (lower(*a.add(i)), lower(*b.add(i))) };
        if ca != cb || ca == 0 {
            return ca as i32 - cb as i32;
        }
    }
    0
}

unsafe extern "C" fn wcsicmp(a: *const u16, b: *const u16) -> i32 {
    unsafe { compare(a, b, usize::MAX) }
}

unsafe extern "C" fn wcsnicmp(a: *const u16, b: *const u16, n: usize) -> i32 {
    unsafe { compare(a, b, n) }
}
