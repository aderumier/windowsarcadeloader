//! Beach Head 2002's and Beach Head 2003 Desert War's cabinet controls: the older
//! `GVRInputRaw.dll` (exports by ordinal: 1 cleanup, 2 init, 3 update) that the games (`BH2.exe`,
//! init for "BH2002"; `desertwar.exe`, "DesertWar") and the multigame shell's control
//! (`ShellControl.ocx`, init for "ShellControl") load. Its drivers (DirectInput
//! joystick, mouse, the Vortek headset's USB tracker) turn the turret: the virtual stick does
//! here, at a speed over time.
//!
//! The caller's state (0x30 bytes):
//!
//! | offset | content |
//! |---|---|
//! | 0x00 / 0x02 | u16 x / y position, 0..0x800 (a full turn, wrapping) |
//! | 0x04 / 0x06 | i16 x / y motion of this update |
//! | 0x08 / 0x0A | previous x / y position |
//! | 0x0C / 0x10 | buttons / previous buttons |
//! | 0x18 / 0x20 | QueryPerformanceCounter of this / the previous update (`gvr_update`) |
//! | 0x28 | QueryPerformanceFrequency |
//!
//! The game turns by the motion (`-console`, the stand-up cabinet) or aims at the position
//! (`-enablevr`, the headset cabinet). The shell moves its selections by the motion, one item per
//! trackball spin past its threshold (it drops what it reads under it): there the stick gives
//! steps, one each [`STEP_REPEAT`] while tilted.
//!
//! Buttons (after the cabinet's `gvrinput.ini` mapping; the shell's names from its
//! `Operator\Buttons` registry):
//!
//! | bit | game | shell |
//! |---|---|---|
//! | 0x1 | missile (Desert War, on release) | up |
//! | 0x2 | | back, attract advance |
//! | 0x4 | weapon change (machine gun, cannon; on release) | down |
//! | 0x8 | fire | accept |
//! | 0x10 | (the cabinet's fifth button, no visible use) | |
//! | 0x20 | | operator menu |

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::Axis;
use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Native(u32);

const NATIVES: &[(&str, Native)] = &[
    ("up", Native(0x1)),
    ("missile", Native(0x1)),
    ("back", Native(0x2)),
    ("weapon", Native(0x4)),
    ("fire", Native(0x8)),
    ("button5", Native(0x10)),
    ("operator", Native(0x20)),
];

/// Virtual stick -> cabinet, overridden by the profile `native_map`. Start is the trigger (the
/// shell's accept: the cabinet has no start button).
const DEFAULT_MAP: &[(&str, &str)] =
    &[("b1", "fire"), ("b2", "weapon"), ("b3", "missile"), ("start", "fire"), ("test", "operator")];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static LAST: AtomicU32 = AtomicU32::new(u32::MAX);
/// The caller is the shell (`ShellControl`), not the game.
static SHELL: AtomicBool = AtomicBool::new(false);

/// A step of the shell's trackball: past twice its threshold (30, the high score page's
/// letters), one item.
const STEP: i16 = 100;
/// The steps' repeat while the stick stays tilted.
const STEP_REPEAT: Duration = Duration::from_millis(250);
/// The tilt that steps.
const STEP_TILT: i16 = 16384;

/// Per axis, when the stick last stepped (`None`: centered).
static STEPPED: Mutex<[Option<Instant>; 2]> = Mutex::new([None, None]);

/// The shell's motion on an axis: a step on tilting, then each [`STEP_REPEAT`].
fn step(axis: i16, last: &mut Option<Instant>, now: Instant) -> i16 {
    if axis.unsigned_abs() < STEP_TILT as u16 {
        *last = None;
        return 0;
    }
    if last.is_some_and(|t| now.duration_since(t) < STEP_REPEAT) {
        return 0;
    }
    *last = Some(now);
    if axis > 0 { STEP } else { -STEP }
}

/// The game names of this library's callers.
pub(crate) fn is_turret(game: &str) -> bool {
    ["BH2002", "DesertWar", "ShellControl"].iter().any(|g| game.eq_ignore_ascii_case(g))
}

/// The caller's name (`GVRInputRawInit`).
pub(crate) fn set_caller(game: &str) {
    SHELL.store(game.eq_ignore_ascii_case("ShellControl"), Ordering::Relaxed);
}

/// The turret's turning speed at full tilt, positions per second (`WAL_BEACHHEAD_AIM_SPEED`, a
/// full turn is 2048); it tilts at half that (its range is a quarter turn).
fn aim_speed() -> f32 {
    static SPEED: OnceLock<f32> = OnceLock::new();
    *SPEED.get_or_init(|| std::env::var("WAL_BEACHHEAD_AIM_SPEED").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(720.0))
}

/// The sub-position remainders of x and y (slow tilts still move), as f32 bits.
static REST: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// One axis' motion over `seconds`: the tilt times the speed, the remainder kept.
fn motion(axis: i16, seconds: f32, rest: &AtomicU32) -> i16 {
    let total = axis as f32 / 32767.0 * aim_speed() * seconds + f32::from_bits(rest.load(Ordering::Relaxed));
    let moved = total.trunc();
    rest.store(if axis == 0 { 0.0f32 } else { total - moved }.to_bits(), Ordering::Relaxed);
    moved as i16
}

unsafe fn get<T: Copy>(state: *const u8, offset: usize) -> T {
    unsafe { (state.add(offset) as *const T).read_unaligned() }
}

unsafe fn put<T>(state: *mut u8, offset: usize, v: T) {
    unsafe { (state.add(offset) as *mut T).write_unaligned(v) }
}

/// `GVRInputRawInit`: the state cleared, the counter's frequency.
pub(crate) fn init(state: *mut u8) {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    unsafe {
        state.write_bytes(0, 0x30);
        QueryPerformanceFrequency(state.add(0x28) as *mut i64);
    }
}

/// `GVRInputRawUpdate`, after the counters' update.
pub(crate) fn fill(state: *mut u8) {
    let stick = wal_payload_common::input(0);
    let mut buttons = 0;
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP)).for_each_pressed(&stick, |Native(bit)| buttons |= bit);
    unsafe {
        let (now, before, frequency) = (get::<i64>(state, 0x18), get::<i64>(state, 0x20), get::<i64>(state, 0x28));
        let seconds = if before == 0 || frequency <= 0 { 0.0 } else { ((now - before) as f32 / frequency as f32).clamp(0.0, 0.25) };
        let (lx, ly) = (stick.axis(Axis::LeftX), stick.axis(Axis::LeftY).saturating_neg());
        let (dx, dy) = if SHELL.load(Ordering::Relaxed) {
            let now = Instant::now();
            let mut stepped = STEPPED.lock().unwrap_or_else(|e| e.into_inner());
            (step(lx, &mut stepped[0], now), step(ly, &mut stepped[1], now))
        } else {
            (motion(lx, seconds, &REST[0]), motion(ly, seconds / 2.0, &REST[1]))
        };
        let (x, y) = (get::<u16>(state, 0), get::<u16>(state, 2));
        put(state, 0x08, x);
        put(state, 0x0A, y);
        put(state, 0x04, dx);
        put(state, 0x06, dy);
        put(state, 0x00, x.wrapping_add(dx as u16) & 0x7ff);
        put(state, 0x02, y.wrapping_add(dy as u16) & 0x7ff);
        put(state, 0x10, get::<u32>(state, 0x0C));
        put(state, 0x0C, buttons);
    }
    if LAST.swap(buttons, Ordering::Relaxed) != buttons {
        log!("turret: buttons {buttons:#x}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_keeps_fractions() {
        let rest = AtomicU32::new(0);
        // a quarter tilt for 8 updates of a frame: 720 / 4 / 60 = 3 positions each
        let moved: i32 = (0..8).map(|_| motion(32767 / 4 + 1, 1.0 / 60.0, &rest) as i32).sum();
        assert_eq!(moved, 24);
        assert_eq!(motion(0, 1.0 / 60.0, &rest), 0);
        assert_eq!(f32::from_bits(rest.load(Ordering::Relaxed)), 0.0);
    }

    #[test]
    fn shell_steps_repeat() {
        let start = Instant::now();
        let mut last = None;
        assert_eq!(step(8000, &mut last, start), 0);
        assert_eq!(step(-32767, &mut last, start), -STEP);
        assert_eq!(step(-32767, &mut last, start + Duration::from_millis(100)), 0);
        assert_eq!(step(-32767, &mut last, start + Duration::from_millis(260)), -STEP);
        assert_eq!(step(0, &mut last, start + Duration::from_millis(300)), 0);
        assert_eq!(step(32767, &mut last, start + Duration::from_millis(310)), STEP);
    }
}
