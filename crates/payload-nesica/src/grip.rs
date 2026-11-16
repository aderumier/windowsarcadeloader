//! The grip controller: a winmm joystick from player 1's virtual stick.
//!
//! Left 4 Dead: Survivors' cabinet has a grip (an analog stick with buttons, a USB joystick)
//! next to its gun: the Source engine's `inputsystem.dll` reads it with `joyGetPosEx` (move
//! with X/Y, the buttons as `JOY1`...) and shows "I/O error 3" without one. Its winmm imports
//! see joystick 0 only, built from the virtual stick (the real joysticks hidden).
//!
//! Options: `WAL_NESICA_GRIP=<module>` (the module reading it, `inputsystem.dll`),
//! `WAL_NESICA_GRIP_BUTTONS=<virtual button>,...`: the virtual buttons of `JOY1`, `JOY2`...
//! (`b1+start`: either; `none` or empty: never pressed).

use std::ffi::c_void;
use std::sync::OnceLock;

use wal_payload_common::{iat, log};
use wal_protocol::{Axis, button};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

const JOYERR_NOERROR: u32 = 0;
const JOYERR_PARMS: u32 = 165;
const JOYERR_UNPLUGGED: u32 = 167;
/// JOYCAPSA's size (the ANSI structure).
const JOYCAPSA_SIZE: usize = 0x194;
const NAME: &[u8] = b"TAITO Grip Controller\0";

static BUTTONS: OnceLock<Vec<u32>> = OnceLock::new();

fn buttons() -> &'static [u32] {
    BUTTONS.get_or_init(|| {
        std::env::var("WAL_NESICA_GRIP_BUTTONS")
            .unwrap_or_default()
            .split(',')
            .map(|item| item.split('+').filter_map(|b| button::from_name(b.trim())).fold(0, |m, b| m | b))
            .collect()
    })
}

/// One axis, 0..65535: the stick's, else the digital directions at the extremes.
fn axis(value: i16, low: bool, high: bool) -> u32 {
    if value == 0 && low != high {
        return if low { 0 } else { 65535 };
    }
    (value as i32 + 32768) as u32
}

extern "system" fn num_devs() -> u32 {
    16
}

/// `joyGetPosEx(id, JOYINFOEX *)`: dwSize, dwFlags, X, Y, Z, R, U, V, dwButtons,
/// dwButtonNumber, dwPOV, reserved.
extern "system" fn pos_ex(id: u32, info: *mut u32) -> u32 {
    if info.is_null() {
        return JOYERR_PARMS;
    }
    if id != 0 {
        return JOYERR_UNPLUGGED;
    }
    let stick = wal_payload_common::input(0);
    let x = axis(stick.axis(Axis::LeftX), stick.pressed(button::LEFT), stick.pressed(button::RIGHT));
    let y = axis(stick.axis(Axis::LeftY), stick.pressed(button::UP), stick.pressed(button::DOWN));
    let mut pressed = 0u32;
    for (i, mask) in buttons().iter().enumerate().take(32) {
        if *mask != 0 && stick.pressed(*mask) {
            pressed |= 1 << i;
        }
    }
    unsafe {
        *info.add(2) = x;
        *info.add(3) = y;
        for i in 4..8 {
            *info.add(i) = 32767;
        }
        *info.add(8) = pressed;
        *info.add(9) = pressed.count_ones();
        *info.add(10) = 0xFFFF; // JOY_POVCENTERED
    }
    JOYERR_NOERROR
}

/// `joyGetDevCapsA(id, JOYCAPSA *, size)`: two axes (0..65535), 32 buttons.
extern "system" fn dev_caps(id: usize, caps: *mut u8, size: u32) -> u32 {
    if caps.is_null() || (size as usize) < JOYCAPSA_SIZE {
        return JOYERR_PARMS;
    }
    if id != 0 {
        return JOYERR_UNPLUGGED;
    }
    let put = |offset: usize, value: u32| unsafe { (caps.add(offset) as *mut u32).write_unaligned(value) };
    unsafe {
        std::ptr::write_bytes(caps, 0, JOYCAPSA_SIZE);
        (caps as *mut u16).write_unaligned(0x0F6D); // wMid
        (caps.add(2) as *mut u16).write_unaligned(0x0001); // wPid
        std::ptr::copy_nonoverlapping(NAME.as_ptr(), caps.add(4), NAME.len());
    }
    put(0x28, 65535); // wXmax
    put(0x30, 65535); // wYmax
    put(0x3C, 32); // wNumButtons
    put(0x40, 10); // wPeriodMin
    put(0x44, 1000); // wPeriodMax
    put(0x64, 2); // wMaxAxes
    put(0x68, 2); // wNumAxes
    put(0x6C, 32); // wMaxButtons
    JOYERR_NOERROR
}

pub(crate) fn init() {
    let Ok(name) = std::env::var("WAL_NESICA_GRIP") else { return };
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let base = unsafe { GetModuleHandleW(wide.as_ptr()) } as usize;
    if base == 0 {
        log!("grip: {name} not loaded");
        return;
    }
    buttons();
    let hooks: [(&str, *const c_void); 4] = [
        ("joyGetNumDevs", num_devs as *const c_void),
        ("joyGetPosEx", pos_ex as *const c_void),
        ("joyGetPos", pos_unplugged as *const c_void),
        ("joyGetDevCapsA", dev_caps as *const c_void),
    ];
    let n = hooks.iter().filter(|(f, r)| unsafe { iat::hook_module(base, "winmm.dll", f, *r as usize) }.is_some()).count();
    log!("grip: joystick 0 of {name} from player 1 ({n} functions)");
}

extern "system" fn pos_unplugged(_id: u32, _info: *mut u8) -> u32 {
    JOYERR_UNPLUGGED
}
