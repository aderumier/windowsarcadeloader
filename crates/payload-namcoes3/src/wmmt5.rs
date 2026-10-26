//! Wangan Midnight Maximum Tune 5 (2014, 64-bit `wmn5r.exe`, Direct3D 11): the payload is the
//! game's HASP library `hasp_windows_x64_109906.dll` (its import).
//!
//! * Dongle: the HASP memory with this game's cabinet serial ([`SERIAL`], `hasp.rs`).
//! * I/O board: the game's own JVS code on COM3 (`jvs.rs`): wheel, pedals, shift lever,
//!   buttons.
//! * Boot (the cabinet's network): the dongle / network checks of the boot sequence pass
//!   ([`patches`]); the store's card terminal is emulated (`wmmt5_terminal.rs`).
//! * Force feedback: the game keeps the forces of its drive board (off in its settings, it is
//!   never sent) in memory, [`ffb`].
//!
//! The saves (`F:`, `G:`: settings, bookkeeping) are the profile's drives. Their first
//! settings ([`initial_settings`]) leave the absent cabinet devices off.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wal_payload_common::{log, send_output};
use wal_protocol::output;

use crate::patches::patch;
use crate::{GAME_BASE, module};

const HASP: &str = "hasp_windows_x64_109906.dll";
const GAME: &str = "wmn5r.exe";

/// The cabinet serial (HASP memory 0xd00) and the two check bytes 0xd3e / 0xd3f.
pub(crate) const SERIAL: &[u8; 12] = b"272213990002";
pub(crate) const SERIAL_CHECK: [u8; 2] = [0x65, 0x9A];

pub(crate) fn is_wmmt5() -> bool {
    module(HASP) != 0
}

static FFB_LOG: AtomicBool = AtomicBool::new(false);

pub(crate) fn attach(exe: &str, base: usize) {
    if !exe.eq_ignore_ascii_case(GAME) {
        log!("wmmt5: loaded in {exe}, nothing to do");
        return;
    }
    GAME_BASE.store(base, Ordering::Relaxed);
    initial_settings();
    crate::wmmt5_net::init(base);
    crate::wmmt5_terminal::start(base);
    patches(base);
    for (dll, name, f) in [
        ("msvcr100.dll", "_wassert", wassert as *const () as usize),
        ("kernel32.dll", "DebugBreak", debug_break as *const () as usize),
    ] {
        if unsafe { wal_payload_common::iat::hook_module(base, dll, name, f) }.is_none() {
            log!("wmmt5: {name} not hooked");
        }
    }
    // the other cabinet devices (IC card reader, touch panel, drive board) are off in the
    // settings: their ports stay absent
    crate::jvs::init(&[base]);
    FFB_LOG.store(std::env::var("WAL_NAMCOES3_FFB_LOG").is_ok_and(|v| v == "1"), Ordering::Relaxed);
    std::thread::spawn(move || ffb(base));
}

/// The operator settings a new cabinet starts with (`setting.lua`), but the IC card reader,
/// touch panel and drive board off (the game stops on their absent ports otherwise), free
/// play, and the pedals' rest at 0.
const SETTINGS: &[(&str, i32)] = &[
    ("mCalibHandleCenter", 32768),
    ("mCalibAccelCenter", 0),
    ("mCalibBrakeCenter", 0),
    ("mCalibMotorCenter", 511),
    ("mCalibTouchLeft", 2048),
    ("mCalibTouchRight", 14336),
    ("mCalibTouchTop", 14336),
    ("mCalibTouchBottom", 2048),
    ("mCoinChute", 1),
    ("mBuyCardCost", 3),
    ("mGameCost", 2),
    ("mContinueCost", 2),
    ("mFullCourseCost", 4),
    ("mFreePlay", 1),
    ("mPcbId", 0),
    ("mIcCardRw", 0),
    ("mIcCardVender", 0),
    ("mMgCardRw", 0),
    ("mForceFeedback", 0),
    ("mWinsAndRemains", 1),
    ("mEventMode", 0),
    ("mEventModeDist", 0),
    ("mEventModeCount", 4),
    ("mCloseType", 2),
    ("mCloseSun", 96),
    ("mCloseMon", 96),
    ("mCloseTue", 96),
    ("mCloseWed", 96),
    ("mCloseThu", 96),
    ("mCloseFri", 96),
    ("mCloseSat", 96),
    ("mCloseDay", 96),
    ("mTouchPanel", 0),
    ("mGameVol", 10),
    ("mAttractVol", 10),
    ("mOfflineMode", 0),
    ("mClockOffset", 0),
    ("mTimeZoneType", 36),
    ("mTimeDayLight", 0),
    ("mTimeZoneHourOffset", 8),
    ("mTimeZoneMinOffset", 0),
    ("mSummerTimeHourOffset", 0),
    ("mSummerTimeMinOffset", 0),
];

/// Writes the first settings on both drives (`F:\setting.lua.gz`, `G:\setting.lua.gz`, the
/// same file) when there are none.
fn initial_settings() {
    const RULE: &str = "--------------------------------\n";
    let mut text = format!("{RULE}---- setting.lua\n{RULE}\n");
    for (key, value) in SETTINGS {
        text += &format!("{key} = {value}\n");
    }
    text += &format!("{RULE}---- EOF\n{RULE}");
    for drive in ["F", "G"] {
        let path = format!("{drive}:\\setting.lua.gz");
        if std::path::Path::new(&path).exists() {
            continue;
        }
        match std::fs::write(&path, gzip_stored(text.as_bytes())) {
            Ok(()) => log!("wmmt5: {path} written (cabinet devices off, free play)"),
            Err(e) => log!("wmmt5: {path}: {e}"),
        }
    }
}

/// A gzip file holding `data` uncompressed (one stored deflate block).
fn gzip_stored(data: &[u8]) -> Vec<u8> {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    let len = data.len() as u16;
    let mut out = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0xFF];
    out.push(1); // final stored block
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(&(!crc).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// The boot sequence's checks (`game::InfoSystem`, 0x58b000..) and the movie player's.
fn patches(base: usize) {
    // the serial check against the network's ("Dongle Mismatch"): match
    patch("dongle match", base, 0x58c6cc, b"\x01", b"\x00");
    // the network state 3..4 test of the boot sequence: always on the next step
    patch("network state", base, 0x58cac0, b"\x0f\x87\xb4\x00\x00\x00", &[0x90; 6]);
    // the network's error 0x965 (no network) not raised
    patch("network error", base, 0x73782d, b"\xe8\x6e\x41\xab\xff", &[0x90; 5]);
    // the race clock's network time checks: the local time
    patch("network time", base, 0x8a1d72, b"\x74", b"\xeb");
    patch("network time 2", base, 0x8a1d91, b"\x72\x25", &[0x90; 2]);
    patch("network time 3", base, 0x8a1da2, b"\x77\x14", &[0x90; 2]);
    // Media Foundation start failure: no DebugBreak (the movies are skipped)
    patch("movie start failure", base, 0x804872, b"\x79", b"\xeb");
    // the store link counts a peer as linked from a private network address only (else state
    // 3, never linked): the terminal and the cabinet's own packets come from the loopback
    patch("link private address", base, 0x8a56e3, b"\x74\x31", b"\xeb\x31");
    // the store link: one node per address (the cabinet's own packets and the terminal's)
    patch("terminal scan", base, 0x87bd6e, b"\x0f\x84\x88\x00\x00\x00", &[0x90; 6]);
    patch("terminal scan 2", base, 0x87bd77, b"\x74\x60", &[0x90; 2]);
    patch("terminal scan 3", base, 0x87bd7d, b"\x75\x4f", &[0x90; 2]);
}

/// The game's debug asserts (`_wassert`): logged, the game goes on.
unsafe extern "C" fn wassert(message: *const u16, file: *const u16, line: u32) {
    let text = |p: *const u16| {
        if p.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    };
    log!("wmmt5: assert {} ({}:{line})", text(message), text(file));
}

/// The game's failure traps (`DebugBreak`, 2500 calls: a failed Media Foundation call...),
/// which end the process without a debugger: logged (where from), the game goes on.
unsafe extern "system" fn debug_break() {
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if COUNT.fetch_add(1, Ordering::Relaxed) < 50 {
        let mut frames = [std::ptr::null_mut(); 1];
        unsafe extern "system" {
            fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut *mut std::ffi::c_void, hash: *mut u32) -> u16;
        }
        unsafe { RtlCaptureStackBackTrace(1, 1, frames.as_mut_ptr(), std::ptr::null_mut()) };
        let at = (frames[0] as usize).wrapping_sub(crate::game_base());
        log!("wmmt5: DebugBreak from {at:#x} ignored");
    }
}

/// The drive board's forces, written by the game each frame (floats): tire slip, centering
/// spring, friction, collisions (the side: positive from the right). 0x196febc: the speed
/// (km/h, integer).
const FORCES: usize = 0x196f188;

/// A collision force in this range is the road's joints and stripes (a vibration).
const JOINTS: std::ops::RangeInclusive<f32> = 0.209..=0.311;

/// Plays the game's forces: the spring as the centering spring (a light one until the game
/// sets any), a collision as a constant push away from its side, the road joints, stripes and
/// tire slip as a vibration.
fn ffb(base: usize) {
    let mut started = false;
    let mut sent = [None; 3];
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let [slip, spring, _friction, collision] = unsafe { *((base + FORCES) as *const [f32; 4]) };
        let finite = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        let (slip, spring, collision) = (finite(slip), finite(spring), finite(collision));
        started |= spring >= 0.001;
        let spring = if started { spring } else { 0.3 };
        let (constant, joints) = if JOINTS.contains(&collision.abs()) { (0.0, collision.abs()) } else { (-collision, 0.0) };
        let vibration = joints.max(slip.abs());
        let scale = |v: f32| (v * output::FFB_MAX as f32) as i32;
        let values = [(output::FFB_SPRING, scale(spring.max(0.0))), (output::FFB_CONSTANT, scale(constant)), (output::FFB_VIBRATION, scale(vibration))];
        let mut changed = false;
        for (i, (id, value)) in values.into_iter().enumerate() {
            if sent[i] != Some(value) {
                sent[i] = Some(value);
                send_output(0, id, value);
                changed = true;
            }
        }
        if changed && FFB_LOG.load(Ordering::Relaxed) {
            log!("wmmt5 ffb: slip {slip:.3} spring {spring:.3} collision {collision:.3}");
        }
    }
}
