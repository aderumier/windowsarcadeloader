//! Shared by the Raw Thrills game modules: code patches, pedal bytes, the cabinet keypad read
//! from the PC's numeric keypad, the wheel motor force sent to the launcher.

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{log, send_output};
use wal_protocol::{Axis, StickState, output};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_ADD, VK_DECIMAL, VK_MULTIPLY, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD2, VK_NUMPAD3,
    VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9, VK_RETURN,
};

/// A code site and the bytes expected there (another build: left alone).
pub(crate) type Site = (usize, &'static [u8]);

pub(crate) fn matches((site, expected): Site) -> bool {
    unsafe { std::slice::from_raw_parts(site as *const u8, expected.len()) == expected }
}

pub(crate) fn write(site: usize, code: &[u8]) {
    let at = site as *mut u8;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as _, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at, code.len());
        VirtualProtect(at as _, code.len(), old, &mut old);
    }
}

/// Replaces the function at `site` by `mov eax, target; jmp eax`.
pub(crate) fn redirect(site: usize, target: usize) {
    let mut code = [0xB8, 0, 0, 0, 0, 0xFF, 0xE0];
    code[1..5].copy_from_slice(&(target as u32).to_le_bytes());
    write(site, &code);
}

/// A pedal's byte, 1 (released) ..= 255: the calibrations take the smallest value seen other
/// than 0 as the minimum (a real pedal never rests at 0).
pub(crate) fn pedal(stick: &StickState, axis: Axis) -> u8 {
    stick.axis_u8(axis).max(1)
}

/// Keypad keys (1-9, *, 0, #) and their virtual keys on the PC's numeric keypad (NumLock on).
const KEYPAD: [&[u16]; 12] = [
    &[VK_NUMPAD1],
    &[VK_NUMPAD2],
    &[VK_NUMPAD3],
    &[VK_NUMPAD4],
    &[VK_NUMPAD5],
    &[VK_NUMPAD6],
    &[VK_NUMPAD7],
    &[VK_NUMPAD8],
    &[VK_NUMPAD9],
    &[VK_DECIMAL, VK_MULTIPLY],
    &[VK_NUMPAD0],
    &[VK_ADD, VK_RETURN],
];
/// No keypad key pressed.
pub(crate) const KEYPAD_NONE: u8 = 0x0F;

/// Index (0..11: 1-9, *, 0, #) of the first keypad key pressed, [`KEYPAD_NONE`] when none.
pub(crate) fn keypad_key() -> u8 {
    KEYPAD
        .iter()
        .position(|vks| vks.iter().any(|vk| unsafe { GetAsyncKeyState(*vk as i32) } as u16 & 0x8000 != 0))
        .map_or(KEYPAD_NONE, |i| i as u8)
}

/// The wheel motor's constant force, -10000 (pushing left) ..= 10000 (pushing right).
static MOTOR: AtomicI32 = AtomicI32::new(0);

/// Keeps a game's motor force (-1..=1, positive pushing left) for the launcher.
pub(crate) fn set_motor(force: f32) -> f32 {
    let f = if force.is_nan() { 0.0 } else { force.clamp(-1.0, 1.0) };
    MOTOR.store((-f * output::FFB_MAX as f32) as i32, Ordering::Relaxed);
    f
}

/// The sdaemon games' motor force function (`cdecl u8(float)`): kept for the launcher; returns
/// the motor byte the game would send (magnitude x 127, 0x80 set for a negative force).
pub(crate) unsafe extern "C" fn motor_force_byte(force: f32) -> u8 {
    let f = set_motor(force);
    let byte = (f.abs() * 127.0) as u8 & 0x7f;
    if f < 0.0 { byte | 0x80 } else { byte }
}

/// The centering spring's strength, 0 ..= 10000 (games driving one).
static SPRING: AtomicI32 = AtomicI32::new(0);

/// Keeps a game's centering spring strength (0..=1) for the launcher.
pub(crate) fn set_spring(strength: f32) {
    let s = if strength.is_nan() { 0.0 } else { strength.clamp(0.0, 1.0) };
    SPRING.store((s * output::FFB_MAX as f32) as i32, Ordering::Relaxed);
}

/// Sends the motor force (and the spring) on change and every second (a launcher connecting
/// late).
#[derive(Default)]
pub(crate) struct FfbSender {
    sent: Option<(i32, Instant)>,
    spring: Option<(i32, Instant)>,
}

impl FfbSender {
    pub(crate) fn poll(&mut self) {
        let value = MOTOR.load(Ordering::Relaxed);
        let due = match self.sent {
            Some((v, at)) => v != value || at.elapsed() > Duration::from_secs(1),
            None => value != 0,
        };
        if due {
            if self.sent.is_none() {
                log!("ffb: first motor force {value}");
            }
            send_output(0, output::FFB_CONSTANT, value);
            self.sent = Some((value, Instant::now()));
        }
        let spring = SPRING.load(Ordering::Relaxed);
        let due = match self.spring {
            Some((v, at)) => v != spring || at.elapsed() > Duration::from_secs(1),
            None => spring != 0,
        };
        if due {
            send_output(0, output::FFB_SPRING, spring);
            self.spring = Some((spring, Instant::now()));
        }
    }
}
