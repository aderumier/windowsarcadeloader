//! Batman: the RIO board over libusb bulk transfers, with the Cars type 2 report layout.
//! This dump imports the board driver directly; the payload preserves its own reader,
//! parser, calibration and switch edge handling. Switches 7/8/9 are fire/brake/start,
//! unlike Cars. The brake pedal feeds both the analog channel and switch 8.
//! Wheel force: image +0x7ef524, normalized by 1.2, as identified by
//! https://github.com/Boomslangnz/FFBArcadePlugin/blob/master/Game%20Files/Batman.cpp
//! (Boomslangnz, GPL-3.0-or-later).

use crate::common::{Site, matches, write};
use wal_payload_common::log;

// Game-level HASP check: failure sends level loading into the spin at 0x12fb710.
const CABINET_CHECK: Site = (0x1384dc0, &[0x55, 0x8b, 0xec, 0x51, 0xa1, 0x44, 0x0c, 0xa7, 0x01]);
const CABINET_CHECK_CALL: Site = (0x12fb700, &[0xe8, 0xbb, 0x96, 0x08, 0x00, 0x85, 0xc0, 0x75, 0x09]);
const CABINET_MODE: usize = 0x1a70c44;
// This build's own FFB debug display reads the same float identified by FFB Arcade Plugin.
const FORCE_READ: Site = (0x146b422, &[0xf3, 0x0f, 0x10, 0x05, 0x24, 0xf5, 0xab, 0x01]);
const FORCE: usize = 0x1abf524; // image base 0x12d0000 + RVA 0x7ef524

fn wheel_force() -> f32 {
    use windows_sys::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
    let mut value = 0.0f32;
    let mut done = 0;
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            FORCE as *const _,
            (&mut value as *mut f32).cast(),
            size_of::<f32>(),
            &mut done,
        )
    };
    if ok != 0 && done == size_of::<f32>() && value.is_finite() {
        // Positive is force from the right (pushing left), matching common::set_motor.
        (value / 1.2).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

const NATIVES: &[(&str, u8)] = &[
    ("coin1", 0),
    ("coin2", 1),
    ("bill", 2),
    ("service", 3),
    ("test", 4),
    ("volup", 5),
    ("voldown", 6),
    ("fire", 7),
    ("digitalbrake", 8),
    ("start", 9),
    ("view", 10),
    ("button3", 11),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("coin", "coin1"),
    ("service", "service"),
    ("test", "test"),
    ("start", "start"),
    ("b1", "fire"),
    ("b2", "view"),
    ("b3", "digitalbrake"),
    ("b5", "bill"),
    ("up", "volup"),
    ("down", "voldown"),
];

pub(crate) fn init(name: &str) -> bool {
    if !name.eq_ignore_ascii_case("batman") {
        return false;
    }
    if !matches(CABINET_CHECK) || !matches(CABINET_CHECK_CALL) {
        log!("batman: unexpected cabinet check code, not this build, left alone");
        return false;
    }
    // Mode 3 is the primary cabinet dongle; mode 2 is a service key and draws a green overlay.
    write(CABINET_MODE, &3u32.to_le_bytes());
    write(CABINET_CHECK.0, &[0xb8, 1, 0, 0, 0, 0xc3]);
    log!("batman: cabinet dongle check emulated (mode 3), level-loading spin avoided");
    let force: fn() -> f32 = if matches(FORCE_READ) {
        log!("batman: force feedback from wheel force at image+0x7ef524, scale 1/1.2");
        wheel_force
    } else {
        log!("batman: unexpected force address code, force feedback disabled");
        || 0.0
    };
    crate::rio::init("batman", NATIVES, DEFAULT_MAP, &[], true, force);
    true
}
