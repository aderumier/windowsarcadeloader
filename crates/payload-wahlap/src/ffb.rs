//! Force feedback from the board outputs the game sets.
//!
//! * `UsbIO_SetSteeringStatu(&{mode, 255, strength, a, b, 3})`: the steering motor. In a race
//!   `mode` is -1 and `strength` the centering strength, 0 at a standstill up to ~750 at full
//!   speed (the test menu drives the motor with `x * 500 + 500`, 0..=1000): the centering
//!   spring, `strength / 1000` (`WAL_WAHLAP_FFB_SPRING`, percent, default 100).
//! * `UsbIO_SetRoadType(t)`: the ground under the car, 0 road, 1 grass, 2 rough ground,
//!   3 rumble strip, with `UsbIO_OutputCarStatuData(&{speed 0..=1, gear, ..})` (floats): a
//!   vibration while off the road, by ground and speed (`WAL_WAHLAP_FFB_ROAD`, percent,
//!   default 100; 0 off).
//!
//! No left/right force: the game sends none the board would turn into one (its collision
//! data is a direction vector every frame).

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use wal_payload_common::send_output;
use wal_protocol::output;

static SPRING: AtomicI32 = AtomicI32::new(-1);
static VIBRATION: AtomicI32 = AtomicI32::new(-1);
static ROAD: AtomicI32 = AtomicI32::new(0);
/// Car speed, 0..=1 (f32 bits).
static SPEED: AtomicU32 = AtomicU32::new(0);

fn percent(var: &str) -> i32 {
    std::env::var(var).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(100)
}

fn send(last: &AtomicI32, id: u16, value: i32) {
    if last.swap(value, Ordering::Relaxed) != value {
        send_output(0, id, value);
    }
}

/// The steering motor's centering strength (0..=1000).
pub(crate) fn steering(strength: i32) {
    let v = (strength.clamp(0, 1000) as i64 * output::FFB_MAX as i64 * percent("WAL_WAHLAP_FFB_SPRING") as i64 / 100_000) as i32;
    send(&SPRING, output::FFB_SPRING, v.min(output::FFB_MAX));
}

pub(crate) fn road_type(t: i32) {
    ROAD.store(t, Ordering::Relaxed);
    vibration();
}

pub(crate) fn speed(v: f32) {
    SPEED.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    vibration();
}

fn vibration() {
    let ground = match ROAD.load(Ordering::Relaxed) {
        1 => 0.25, // grass
        2 => 0.45, // rough ground
        3 => 0.6,  // rumble strip
        _ => 0.0,
    };
    let speed = f32::from_bits(SPEED.load(Ordering::Relaxed));
    let strength = ground * (0.3 + 0.7 * speed) * percent("WAL_WAHLAP_FFB_ROAD") as f32 / 100.0;
    // steps of 5%: no message flood at each speed change
    let v = ((strength * 20.0).round() / 20.0 * output::FFB_MAX as f32) as i32;
    send(&VIBRATION, output::FFB_VIBRATION, v.clamp(0, output::FFB_MAX));
}
