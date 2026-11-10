//! A 6-speed H shifter driven from the virtual stick (Initial D's cabinets): its gear moved one
//! gear by each press of b7 (up) / b6 (down), neutral 0, or held on kp1-kp6 (an H shifter's
//! positions, segatools' positional shifter: that gear while held, neutral when released).
//! The cabinet reads the shifter's gate switches: up, down, left, right (1st up + left, 2nd
//! down + left, 3rd up, 4th down, 5th up + right, 6th down + right).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wal_protocol::button as b;

static GEAR: AtomicU8 = AtomicU8::new(0);
static GEAR_HELD: AtomicU8 = AtomicU8::new(0);
static POSITIONAL: AtomicBool = AtomicBool::new(false);

/// The gear (0 neutral, 1-6) for player 1's buttons at this read.
pub fn gear(buttons: u32) -> u8 {
    if let Some(n) = (1..=6u8).find(|&n| buttons & (b::KP0 << n) != 0) {
        POSITIONAL.store(true, Ordering::Relaxed);
        GEAR.store(n, Ordering::Relaxed);
        return n;
    }
    if POSITIONAL.swap(false, Ordering::Relaxed) {
        GEAR.store(0, Ordering::Relaxed);
    }
    let held = (buttons & b::B7 != 0) as u8 | ((buttons & b::B6 != 0) as u8) << 1;
    let pressed = held & !GEAR_HELD.swap(held, Ordering::Relaxed);
    let mut g = GEAR.load(Ordering::Relaxed);
    if pressed & 1 != 0 && g < 6 {
        g += 1;
    }
    if pressed & 2 != 0 && g > 0 {
        g -= 1;
    }
    GEAR.store(g, Ordering::Relaxed);
    g
}

/// The gate switches of a gear as a JVS switch byte: up 0x20, down 0x10, left 0x08, right 0x04.
pub fn gate(gear: u8) -> u8 {
    const UP: u8 = 0x20;
    const DOWN: u8 = 0x10;
    const LEFT: u8 = 0x08;
    const RIGHT: u8 = 0x04;
    match gear {
        1 => UP | LEFT,
        2 => DOWN | LEFT,
        3 => UP,
        4 => DOWN,
        5 => UP | RIGHT,
        6 => DOWN | RIGHT,
        _ => 0,
    }
}
