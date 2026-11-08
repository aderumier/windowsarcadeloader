//! The cabinet's controls read as winmm joystick 0 (`WAL_APM3_JOYSTICK=1`: Tapping Skill Test,
//! whose game and test program never call the system's switch API: `joyGetPosEx(0)`'s buttons
//! are the I/O board's). Each switch is a joystick button: `WAL_APM3_JOY_MAP`
//! `<native>:<bit>,...` (the natives of `WAL_MAP`, bits 0..31), merged over the default map.

use std::sync::OnceLock;

use wal_payload_common::{iat, log};

const JOYERR_NOERROR: u32 = 0;
const JOYERR_UNPLUGGED: u32 = 167;

/// The default map: native, joystick button bit. The APM3 I/O board is an IO4 (USB HID): its
/// first button word is bits 0..15, its second bits 16..31 (segatools' `apm3hook/io4.c`).
const DEFAULT: &[(&str, u32)] = &[
    ("test", 9),
    ("service", 6),
    ("home", 28),
    ("start", 7),
    ("up", 5),
    ("down", 4),
    ("left", 3),
    ("right", 2),
    ("push01", 1),
    ("push02", 0),
    ("push03", 15),
    ("push04", 14),
    ("push05", 13),
    ("push06", 12),
    ("push07", 11),
    ("push08", 10),
];

/// For each APM3 switch id (bit of `read_switches`), its joystick button bit.
static MAP: OnceLock<Vec<(u32, u32)>> = OnceLock::new();

pub(crate) fn enabled() -> bool {
    crate::env("WAL_APM3_JOYSTICK").is_some_and(|v| v == "1")
}

fn map() -> &'static [(u32, u32)] {
    MAP.get_or_init(|| {
        let mut pairs: Vec<(String, u32)> = DEFAULT.iter().map(|&(n, b)| (n.to_string(), b)).collect();
        for item in crate::env("WAL_APM3_JOY_MAP").unwrap_or_default().split(',').filter(|s| !s.trim().is_empty()) {
            match item.split_once(':').and_then(|(n, b)| Some((n.trim().to_ascii_lowercase(), b.trim().parse::<u32>().ok().filter(|b| *b < 32)?))) {
                Some((name, bit)) => {
                    pairs.retain(|(n, _)| *n != name);
                    pairs.push((name, bit));
                }
                None => log!("joystick: WAL_APM3_JOY_MAP: {item:?} ignored (<native>:<bit 0..31>)"),
            }
        }
        let map: Vec<(u32, u32)> = pairs.iter().filter_map(|(n, b)| Some((crate::native_id(n)?, *b))).collect();
        log!("joystick: the switches on joystick 0: {pairs:?}");
        map
    })
}

/// `joyGetPosEx(id, JOYINFOEX *)`: joystick 0 is the board (axes centered, its buttons).
unsafe extern "system" fn joy_get_pos_ex(id: u32, info: *mut u32) -> u32 {
    if id != 0 || info.is_null() {
        return JOYERR_UNPLUGGED;
    }
    let switches = crate::read_switches();
    let buttons = map().iter().filter(|(id, _)| switches & (1 << id) != 0).fold(0u32, |b, (_, bit)| b | 1 << bit);
    unsafe {
        // dwXpos..dwVpos, dwButtons, dwButtonNumber, dwPOV
        for i in 2..8 {
            *info.add(i) = 0x7fff;
        }
        *info.add(8) = buttons;
        *info.add(9) = buttons.count_ones();
        *info.add(10) = 0xffff;
    }
    JOYERR_NOERROR
}

/// `joyGetDevCapsW(id, JOYCAPSW *, size)`: joystick 0 has 32 buttons, 6 axes of 0..0xffff.
unsafe extern "system" fn joy_get_dev_caps_w(id: usize, caps: *mut u8, size: u32) -> u32 {
    if id != 0 || caps.is_null() {
        return JOYERR_UNPLUGGED;
    }
    unsafe {
        std::ptr::write_bytes(caps, 0, size as usize);
        // JOYCAPSW: wMid, wPid, szPname[32], then wXmin.. (u32 each)
        let put = |at: usize, v: u32| {
            if at + 4 <= size as usize {
                (caps.add(at) as *mut u32).write_unaligned(v)
            }
        };
        let name: Vec<u16> = "APM3 I/O".encode_utf16().collect();
        for (i, c) in name.iter().enumerate() {
            (caps.add(4 + 2 * i) as *mut u16).write_unaligned(*c);
        }
        // wXmin, wXmax, wYmin, wYmax, wZmin, wZmax, wNumButtons, wPeriodMin, wPeriodMax
        for (i, v) in [0, 0xffff, 0, 0xffff, 0, 0xffff, 32, 10, 1000].into_iter().enumerate() {
            put(68 + 4 * i, v);
        }
        // wRmin.. wVmax, wCaps, wMaxAxes, wNumAxes, wMaxButtons
        for (i, v) in [0, 0xffff, 0, 0xffff, 0, 0xffff, 0, 6, 6, 32].into_iter().enumerate() {
            put(104 + 4 * i, v);
        }
    }
    JOYERR_NOERROR
}

pub(crate) fn start() {
    map();
    unsafe {
        iat::hook("winmm.dll", "joyGetPosEx", joy_get_pos_ex as *const () as usize);
        iat::hook("winmm.dll", "joyGetDevCapsW", joy_get_dev_caps_w as *const () as usize);
    }
    log!("joystick: joystick 0 is the cabinet's I/O board");
}
