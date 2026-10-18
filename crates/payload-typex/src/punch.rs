//! Sonic Blast Heroes' punching pad (`WAL_TYPEX_PUNCH=sonic-blast-heroes`).
//!
//! The cabinet's pad reports a punch through a custom XIO node (`GWXioNode_Hero`): pad sensor
//! switches and the time the pad takes to pass its sensors (the force is about C / time²,
//! then a calibration table). The game also has a keyboard pad (`CPunchingPadEmu`, used when
//! XIO is absent: a punch is F4 held) and a keyboard safety sensor (`CHumanSensorEmu`: the
//! light curtain around the pad, broken while F9 is held). Both are forced, the pad's key
//! queries answered from the virtual sticks and its force replaced:
//!
//! * the punch is held while `accel` (a gamepad trigger) is past `PRESS` or `b1` is held;
//! * its force is the trigger's peak during the punch (linear, up to
//!   `WAL_TYPEX_PUNCH_MAX_KGF`, default 200 kgf), `b1` the full force;
//! * the force becomes the pad's raw value found with the game's own conversion (its
//!   `GetKgf`, called on a scratch pad: a binary search, the value decreasing as the force
//!   grows), so the screens and rankings show that force.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wal_payload_common::log;
use wal_protocol::{Axis, button};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// Trigger travel (0..=32767) above which the punch is held.
const PRESS: i32 = 6000;

/// `CHumanSensorAdapter` init: `je` taking the keyboard sensor when XIO is absent -> `jmp`.
const SENSOR_EMU: (usize, [u8; 2]) = (0x2C468, [0x74, 0xDA]);
/// `CPunchingPadAdapter` init: `je` taking the keyboard pad when XIO is absent -> `jmp`.
const PAD_EMU: (usize, [u8; 2]) = (0x32435, [0x74, 0x3C]);
/// The keyboard sensor's F9 query (`call` key held).
const SENSOR_KEY: usize = 0x2C325;
/// The keyboard pad's F4 queries: `call` key held / pressed / released (RVA, game function).
const HELD: [usize; 3] = [0x31DB5, 0x31DEE, 0x31E44];
const PRESSED: usize = 0x31E05;
const RELEASED: usize = 0x31E28;
const KEY_HELD: usize = 0x8D020;
const KEY_PRESSED: usize = 0x8CFF0;
const KEY_RELEASED: usize = 0x8D050;
/// `CPunchingPadEmu` vtable slot of its punch end (RVA 0x31C50: raw value from F4's hold time).
const PAD_END_SLOT: (usize, usize) = (0x23C370, 0x31C50);
/// `CPunchingPadBase::GetKgf` (thiscall, the raw value at +0x14): the force x 1000.
const GET_KGF: usize = 0x31890;

static MAX_KGF: AtomicU32 = AtomicU32::new(200);
/// Peak force of the punch in progress (0..=1000 per mille) and of the last ended one.
static PEAK: AtomicU32 = AtomicU32::new(0);
static LAST: AtomicU32 = AtomicU32::new(0);
static WAS_PRESSED: AtomicBool = AtomicBool::new(false);
static WAS_RELEASED: AtomicBool = AtomicBool::new(false);
static BASE: AtomicU32 = AtomicU32::new(0);

/// The punch held now (any player: one pad), its peak force updated.
fn held() -> bool {
    let mut down = false;
    for player in 0..2 {
        let stick = wal_payload_common::input(player);
        let force = if stick.pressed(button::B1) {
            1000
        } else {
            let travel = stick.axis(Axis::Accel) as i32;
            if travel < PRESS { 0 } else { (travel.min(32767) * 1000 / 32767) as u32 }
        };
        if force > 0 {
            down = true;
            PEAK.fetch_max(force, Ordering::Relaxed);
        }
    }
    down
}

unsafe extern "C" fn key_held(_key: u32) -> u32 {
    held() as u32
}

unsafe extern "C" fn key_pressed(_key: u32) -> u32 {
    let now = held();
    let was = WAS_PRESSED.swap(now, Ordering::Relaxed);
    (now && !was) as u32
}

unsafe extern "C" fn key_released(_key: u32) -> u32 {
    let now = held();
    let was = WAS_RELEASED.swap(now, Ordering::Relaxed);
    if was && !now {
        LAST.store(PEAK.swap(0, Ordering::Relaxed), Ordering::Relaxed);
        return 1;
    }
    0
}

unsafe extern "C" fn never(_key: u32) -> u32 {
    0
}

/// The game's force (x 1000) of a raw pad value. Its float to integer conversion overflows
/// (0x80000000) for the shortest times: beyond any force.
fn kgf_milli(raw: u32) -> i32 {
    let mut pad = [0u32; 32];
    pad[5] = raw;
    let f: unsafe extern "thiscall" fn(*mut u32) -> i32 =
        unsafe { std::mem::transmute(BASE.load(Ordering::Relaxed) as usize + GET_KGF) };
    match unsafe { f(pad.as_mut_ptr()) } {
        v if v < 0 => i32::MAX,
        v => v,
    }
}

/// The largest raw value whose force reaches `target` (x 1000).
fn raw_for(target: i32) -> u32 {
    let (mut lo, mut hi) = (1u32, 200_000u32);
    if kgf_milli(lo) < target {
        return lo;
    }
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if kgf_milli(mid) >= target { lo = mid } else { hi = mid - 1 }
    }
    lo
}

/// `CPunchingPadEmu`'s punch end (thiscall): its resets, then the raw value of our force.
unsafe extern "thiscall" fn pad_end(pad: *mut u32) {
    let peak = match LAST.swap(0, Ordering::Relaxed) {
        0 => PEAK.swap(0, Ordering::Relaxed),
        last => last,
    };
    let target = (MAX_KGF.load(Ordering::Relaxed) as i64 * 1000 * peak.max(1) as i64 / 1000) as i32;
    let raw = raw_for(target.max(1));
    unsafe {
        *pad.add(4) = 0; // state
        *pad.add(2) = 0;
        *pad.add(3) = 0;
        *pad.add(7) &= !8;
        *pad.add(5) = raw;
    }
    log!("punch: force {} per mille -> raw {} ({} g)", peak, raw, kgf_milli(raw));
}

unsafe fn write(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const _, bytes.len(), old, &mut old);
    }
}

/// The `call` at `site` calls `expected` (RVAs).
unsafe fn calls(base: usize, site: usize, expected: usize) -> bool {
    let at = base + site;
    unsafe { *(at as *const u8) == 0xE8 && (at + 5).wrapping_add(std::ptr::read_unaligned((at + 1) as *const i32) as usize) == base + expected }
}

unsafe fn redirect(base: usize, site: usize, f: usize) {
    let at = base + site;
    unsafe { write(at + 1, &(f.wrapping_sub(at + 5) as i32).to_le_bytes()) };
}

pub(crate) fn init() {
    if !std::env::var("WAL_TYPEX_PUNCH").is_ok_and(|v| v.trim() == "sonic-blast-heroes") {
        return;
    }
    if let Some(max) = std::env::var("WAL_TYPEX_PUNCH_MAX_KGF").ok().and_then(|v| v.trim().parse().ok()) {
        MAX_KGF.store(max, Ordering::Relaxed);
    }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    BASE.store(base as u32, Ordering::Relaxed);
    let code = |(rva, bytes): (usize, [u8; 2])| unsafe { std::slice::from_raw_parts((base + rva) as *const u8, 2) == bytes };
    let sites = HELD.iter().all(|&s| unsafe { calls(base, s, KEY_HELD) })
        && unsafe { calls(base, PRESSED, KEY_PRESSED) && calls(base, RELEASED, KEY_RELEASED) && calls(base, SENSOR_KEY, KEY_HELD) };
    let slot = base + PAD_END_SLOT.0;
    if !code(SENSOR_EMU) || !code(PAD_EMU) || !sites || unsafe { *(slot as *const usize) } != base + PAD_END_SLOT.1 {
        log!("punch: unknown executable, pad left alone");
        return;
    }
    unsafe {
        write(base + SENSOR_EMU.0, &[0xEB]);
        write(base + PAD_EMU.0, &[0xEB]);
        redirect(base, SENSOR_KEY, never as *const () as usize);
        for s in HELD {
            redirect(base, s, key_held as *const () as usize);
        }
        redirect(base, PRESSED, key_pressed as *const () as usize);
        redirect(base, RELEASED, key_released as *const () as usize);
        write(slot, &(pad_end as *const () as usize).to_le_bytes());
    }
    log!("punch: keyboard pad and safety sensor forced, punch = accel / b1, max {} kgf", MAX_KGF.load(Ordering::Relaxed));
}
