//! Force feedback computed from the game's memory (`WAL_TYPEX_FFB=<game>`), sent to the launcher
//! as [`output`] states (the launcher plays them on player 1's wheel or gamepad). A thread reads
//! the game every 16 ms and sends the effects on change, and again every second.
//!
//! `battle-gear-4` (Battle Gear 4 Tuned 2.08): the race flag (+0x4A9508), the wall and car
//! contact flags (+0x42EBB2, +0x42EBB3) and the speed (float, +0x3F3000, km/h). In a race:
//! centering spring; a wall or car hit on the left (wall 0x10, car 0x08) pushes the wheel to the
//! right, on the right (wall 0x20, car 0x02) to the left, with a force of speed / 180 (full from
//! 180 km/h); a car contact (0x01) vibrates as much. `WAL_TYPEX_FFB_SPRING` (0-100, default
//! 100) scales the spring.
//!
//! `chase-hq-2` (Chase H.Q. 2): the wheel motor command of the game's I/O output word (the
//! 32-bit value at [+0x130B558] + 0x45), lamp bits (0x4001, 0x10, 0x400, 0x200, 0x80, 0x08,
//! 0x100) cleared: 15 levels each way (codes below), levels 16-30 push to the right with
//! (31 - level) / 15 of full force, 1-15 to the left with (16 - level) / 15.
//!
//! `WAL_TYPEX_FFB_TRACE=1` logs the values read when they change.

use std::time::{Duration, Instant};

use wal_payload_common::{log, send_output};
use wal_protocol::output;
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub(crate) fn init() {
    let Ok(game) = std::env::var("WAL_TYPEX_FFB") else { return };
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let trace = std::env::var("WAL_TYPEX_FFB_TRACE").is_ok_and(|v| v == "1");
    match game.trim() {
        "battle-gear-4" => {
            let spring = std::env::var("WAL_TYPEX_FFB_SPRING").ok().and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(100).clamp(0, 100);
            log!("ffb: Battle Gear 4 (spring {spring}%)");
            let mut game = BattleGear4 { base, spring, trace, racing: false, traced: (0, 0, 0, 0) };
            std::thread::spawn(move || run(|| game.effects()));
        }
        "chase-hq-2" => {
            log!("ffb: Chase H.Q. 2");
            let mut game = ChaseHq2 { base, trace, traced: -1 };
            std::thread::spawn(move || run(|| game.effects()));
        }
        other => log!("ffb: unknown game {other:?}"),
    }
}

/// Effect states sent to the launcher: constant, spring, vibration.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
struct Effects {
    constant: i32,
    spring: i32,
    vibration: i32,
}

/// Reads the game every 16 ms, sends the effects on change and every second (a launcher
/// connecting late gets the state).
fn run(mut effects: impl FnMut() -> Effects) {
    let mut sent: Option<Effects> = None;
    let mut last_send = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let e = effects();
        let refresh = last_send.elapsed() > Duration::from_secs(1);
        if sent == Some(e) && !refresh {
            continue;
        }
        let old = sent.unwrap_or(Effects { constant: i32::MIN, spring: i32::MIN, vibration: i32::MIN });
        for (id, value, before) in [
            (output::FFB_CONSTANT, e.constant, old.constant),
            (output::FFB_SPRING, e.spring, old.spring),
            (output::FFB_VIBRATION, e.vibration, old.vibration),
        ] {
            if refresh || value != before {
                send_output(0, id, value);
            }
        }
        sent = Some(e);
        last_send = Instant::now();
    }
}

/// `size_of::<T>()` bytes at `address` of the game, None when unreadable (a pointer not set
/// yet).
fn read<T: Copy + Default>(address: usize) -> Option<T> {
    let mut value = T::default();
    let mut done = 0;
    let ok = unsafe {
        ReadProcessMemory(GetCurrentProcess(), address as *const _, (&mut value as *mut T).cast(), size_of::<T>(), &mut done)
    };
    (ok != 0 && done == size_of::<T>()).then_some(value)
}

struct BattleGear4 {
    base: usize,
    spring: i32,
    trace: bool,
    racing: bool,
    traced: (u8, u8, u8, i32),
}

impl BattleGear4 {
    fn effects(&mut self) -> Effects {
        let racing = read::<u8>(self.base + 0x4A9508).unwrap_or(0) != 0;
        let wall = read::<u8>(self.base + 0x42EBB2).unwrap_or(0);
        let car = read::<u8>(self.base + 0x42EBB3).unwrap_or(0);
        let speed = read::<f32>(self.base + 0x3F3000).unwrap_or(0.0);
        if racing != self.racing {
            log!("ffb: race {}", if racing { "started" } else { "ended" });
            self.racing = racing;
        }
        if self.trace {
            let now = (racing as u8, wall, car, speed as i32 / 10 * 10);
            if now != self.traced {
                log!("ffb: race {} wall {wall:#04x} car {car:#04x} speed {speed:.0}", racing as u8);
                self.traced = now;
            }
        }
        let force = if speed.is_finite() { ((speed / 180.0).clamp(0.0, 1.0) * output::FFB_MAX as f32) as i32 } else { 0 };
        let mut e = Effects::default();
        if racing {
            e.spring = output::FFB_MAX * self.spring / 100;
            if wall & 0x10 != 0 || car & 0x08 != 0 {
                e.constant = force;
            } else if wall & 0x20 != 0 || car & 0x02 != 0 {
                e.constant = -force;
            }
            if car & 0x01 != 0 {
                e.vibration = force;
            }
        }
        e
    }
}

struct ChaseHq2 {
    base: usize,
    trace: bool,
    traced: i32,
}

/// Chase H.Q. 2's wheel motor codes (lamp bits cleared), level 30 down to 1.
const CHASE_HQ_2_CODES: [i32; 30] = [
    28672, 24640, 28736, 16624, 30720, 26688, 30784, 24608, 28704, 24672, 28768, 26656, 30752, 26720, 30816,
    20480, 16448, 20544, 18432, 22528, 18496, 22592, 16416, 20512, 16480, 20576, 18464, 22560, 18528, 22624,
];

impl ChaseHq2 {
    fn effects(&mut self) -> Effects {
        let Some(raw) = read::<u32>(self.base + 0x130B558).and_then(|p| read::<i32>(p as usize + 0x45)) else {
            return Effects::default();
        };
        let mut code = raw;
        for lamp in [0x4001, 0x10, 0x400, 0x200, 0x80, 0x08, 0x100] {
            if code & lamp == lamp {
                code -= lamp;
            }
        }
        let level = CHASE_HQ_2_CODES.iter().position(|c| *c == code).map_or(0, |i| 30 - i as i32);
        if self.trace && level != self.traced {
            log!("ffb: output {raw:#x} motor code {code} level {level}");
            self.traced = level;
        }
        let constant = match level {
            16..=30 => output::FFB_MAX * (31 - level) / 15,
            1..=15 => -(output::FFB_MAX * (16 - level) / 15),
            _ => 0,
        };
        Effects { constant, ..Effects::default() }
    }
}
