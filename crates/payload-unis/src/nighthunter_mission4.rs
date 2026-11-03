//! Night Hunter 3.0.2's fourth mission on the mission select (`WAL_UNIS_NIGHTHUNTER_MISSION4=1`,
//! the 3.0.2 profile's).
//!
//! The game plays its fourth mission (scene 6) last, without the select, once the other three
//! are cleared; the profile's config (`MUST_PASS_LEVEL_4 = YES`) makes it selectable from the
//! start, but the select scene (`level0`, `Panel_head/select`) shows three islands: the fourth's
//! (`image_level4`, its highlight `show4`, its label `label4`) is inactive, placed for an older
//! map (over the third island).
//!
//! When the player opens `level0` (the executable's `CreateFileW`), the payload gives it a copy
//! beside the executable: those three objects active, the island at the map's bottom right (300
//! wide), its label below it. A `GameObject`'s `m_IsActive` is its last byte; a
//! `RectTransform`'s `m_AnchoredPosition` and `m_SizeDelta` its bytes 84 and 92. The file is
//! checked first (its size, each patched value's original): another dump's is left alone.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

/// 3.0.2's `level0` size.
const LEVEL0_SIZE: usize = 2_635_356;

/// The `GameObject`s made active: their `m_IsActive` byte's offset in `level0`.
const ACTIVATE: &[(&str, usize)] = &[("image_level4", 220_224 + 75 - 1), ("show4", 152_368 + 83 - 1), ("label4", 191_848 + 59 - 1)];

/// A `RectTransform` (its offset in `level0`, its original anchored position) placed anew:
/// (position, size).
struct Place {
    at: usize,
    was: (f32, f32),
    pos: (f32, f32),
    size: (f32, f32),
}

const PLACES: &[Place] = &[
    // the island (380 x 330 scaled), its highlight (the same scale), the label below
    Place { at: 2_244_576, was: (354.0, 179.0), pos: (520.0, -300.0), size: (300.0, 260.0) },
    Place { at: 2_171_808, was: (354.0, 179.0), pos: (520.0, -300.0), size: (316.0, 267.0) },
    Place { at: 2_213_640, was: (421.6, 74.6), pos: (520.0, -460.0), size: (194.0, 62.0) },
];

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn put_f32(b: &mut [u8], at: usize, v: f32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The select scene with the fourth island, or None (not 3.0.2's).
pub fn fourth_island(file: &[u8]) -> Option<Vec<u8>> {
    if file.len() != LEVEL0_SIZE {
        log!("nighthunter: level0 not 3.0.2's ({} bytes): left alone", file.len());
        return None;
    }
    let mut out = file.to_vec();
    for &(name, at) in ACTIVATE {
        if out[at] != 0 {
            log!("nighthunter: level0's {name} not the expected object");
            return None;
        }
        out[at] = 1;
    }
    for p in PLACES {
        let pos = (f32_at(file, p.at + 84)?, f32_at(file, p.at + 88)?);
        if (pos.0 - p.was.0).abs() > 0.1 || (pos.1 - p.was.1).abs() > 0.1 {
            log!("nighthunter: level0's RectTransform at {} not the expected one", p.at);
            return None;
        }
        for (i, v) in [p.pos.0, p.pos.1, p.size.0, p.size.1].into_iter().enumerate() {
            put_f32(&mut out, p.at + 84 + 4 * i, v);
        }
    }
    Some(out)
}

/// The copy's path (UTF-16, NUL terminated), once made; None: the original opened.
static LEVEL0: OnceLock<Option<Vec<u16>>> = OnceLock::new();
static ORIG: AtomicUsize = AtomicUsize::new(0);

fn copy(original: &str) -> Option<Vec<u16>> {
    let patched = fourth_island(&std::fs::read(original).ok()?)?;
    let path = std::env::current_exe().ok()?.with_file_name("wal-level0");
    std::fs::write(&path, patched).ok()?;
    log!("nighthunter: the fourth mission on the select ({})", path.display());
    Some(path.as_os_str().to_str()?.encode_utf16().chain([0]).collect())
}

type P = *mut c_void;

unsafe extern "system" fn create_file_w(name: *const u16, access: u32, share: u32, security: P, disposition: u32, flags: u32, template: P) -> P {
    let orig: unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P = unsafe { std::mem::transmute(ORIG.load(Ordering::Relaxed)) };
    let mut target = name;
    if !name.is_null() && access & 0x4000_0000 == 0 {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) });
        if path.to_ascii_lowercase().replace('/', "\\").ends_with("_data\\level0")
            && let Some(copy) = LEVEL0.get_or_init(|| copy(&path)).as_ref()
        {
            target = copy.as_ptr();
        }
    }
    unsafe { orig(target, access, share, security, disposition, flags, template) }
}

pub fn init() {
    if !std::env::var("WAL_UNIS_NIGHTHUNTER_MISSION4").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "CreateFileW", create_file_w as *const () as usize) } {
        ORIG.store(o, Ordering::Relaxed);
    }
}
