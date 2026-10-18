//! The game's winmm joystick API sees no joystick (every call `JOYERR_UNPLUGGED`), unless
//! `WAL_JOYSTICK_DISABLE=0`. A cabinet has no PC game controller,
//! its controls are the I/O board's, but some arcade builds also read one with `joyGetPos(Ex)`
//! next to the board: Ge-Sen Love Plus Pengo!'s ship drifted right from a pad's axis at rest
//! (a trigger at its minimum), Phantom Breaker: Another Code read joysticks 0 and 1 as its
//! players in game, while their test menus, reading the board, showed every input right.

use crate::{iat, log};

const JOYERR_UNPLUGGED: u32 = 167;

/// `joyGetNumDevs`: the joysticks the driver supports, 16 as on Windows, whatever is plugged
/// (0 means no joystick driver: Music GunGun! 2 stopped on "DirectInput initialization failed").
extern "system" fn joy_get_num_devs() -> u32 {
    16
}

extern "system" fn joy_unplugged_2(_id: u32, _info: *mut u8) -> u32 {
    JOYERR_UNPLUGGED
}

extern "system" fn joy_unplugged_3(_id: u32, _caps: *mut u8, _size: u32) -> u32 {
    JOYERR_UNPLUGGED
}

pub fn init() {
    if std::env::var("WAL_JOYSTICK_DISABLE").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let hooks: [(&str, usize); 6] = [
        ("joyGetNumDevs", joy_get_num_devs as *const () as usize),
        ("joyGetPos", joy_unplugged_2 as *const () as usize),
        ("joyGetPosEx", joy_unplugged_2 as *const () as usize),
        ("joyGetDevCapsA", joy_unplugged_3 as *const () as usize),
        ("joyGetDevCapsW", joy_unplugged_3 as *const () as usize),
        ("joySetCapture", joy_unplugged_3 as *const () as usize),
    ];
    let mut n = 0;
    for (name, f) in hooks {
        if unsafe { iat::hook("winmm.dll", name, f) }.is_some() {
            n += 1;
        }
    }
    log!("joystick: winmm joysticks hidden ({n} functions)");
}
