//! The Swarm (PC port of the cabinet game): its input adapter, `Local\SwarmPcInputV1`.
//!
//! The dump's `the_swarm_controls.exe` reads no USBIO board, mouse or keyboard: its .NET
//! adapter (`Swarm PC Fix/SwarmInput.exe`) reads the guns and writes them to a 512-byte shared
//! memory the game polls. `WAL_GLOBALVR_SWARM=1` makes the payload that adapter, fed by the
//! virtual sticks (gun position on the left stick):
//!
//! | offset | content |
//! |---|---|
//! | 0 | magic `0x53575043` ("CPWS") |
//! | 4 | 1: input active (written 0 while the players are updated) |
//! | 8 | tick count |
//! | 16 + 16 * player | aim X, Y (f32, 0..1 of the window), buttons (bit 0 fire, 1 grenade, 2 shield) |
//! | 64 | 1: a command text is at 128 (the game clears it once read) |
//! | 68 | 1: the game asks for the adapter's test menu (cleared, no menu) |
//! | 72 | start requests, bit per player (the game clears them) |
//! | 80 | game: 1 once its window is fullscreen |
//! | 84 | game: 1 in the front end (title, stage selection) |
//! | 128 | command text: console commands separated by `\n`, NUL terminated, < 350 bytes |
//!
//! In game the buttons go as console commands on their changes (`+char_attack P`,
//! `-char_attack P`, `char_throw P`, `+char_block P 2 2`, `-char_block P 2 2`); in the front
//! end the aim goes every 33 ms as `scriptRun UI:PcFrontendAim(P,X,Y,FIRE)`, the buttons as 0.
//! A fire held when the front end ends is ignored until released (the stage selection shot
//! would otherwise fire in game).
//!
//! Stage changes (a run of the game per stage): `swarm_campaign.rs`.
//!
//! `WAL_GLOBALVR_SWARM_TRACE=1` logs the commands sent, the aim and buttons once a second, and a
//! command text the game has not read after 2 s.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE};
use windows_sys::Win32::System::SystemInformation::GetTickCount;

const NAME: &str = "Local\\SwarmPcInputV1";
const SIZE: usize = 512;
const MAGIC: i32 = 1398231107;
const TEXT_MAX: usize = 350;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    Fire,
    Grenade,
    Shield,
    Start,
}

const NATIVES: &[(&str, Native)] =
    &[("fire", Native::Fire), ("grenade", Native::Grenade), ("shield", Native::Shield), ("start", Native::Start)];

/// Virtual stick -> adapter buttons, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[("b1", "fire"), ("b2", "grenade"), ("b3", "shield"), ("start", "start")];

const FIRE: u32 = 1;
const GRENADE: u32 = 2;
const SHIELD: u32 = 4;

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static TRACE: OnceLock<bool> = OnceLock::new();

fn trace() -> bool {
    *TRACE.get_or_init(|| std::env::var("WAL_GLOBALVR_SWARM_TRACE").is_ok_and(|v| v == "1"))
}

pub(crate) fn init() {
    if std::env::var("WAL_GLOBALVR_SWARM").map_or(true, |v| v != "1") {
        return;
    }
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    // Created before the game runs: it opens the mapping at startup.
    let name: Vec<u16> = NAME.encode_utf16().chain(Some(0)).collect();
    let mem = unsafe {
        let h = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, SIZE as u32, name.as_ptr());
        if h.is_null() {
            log!("swarm: CreateFileMapping {NAME} failed");
            return;
        }
        MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, SIZE).Value as *mut u8
    };
    if mem.is_null() {
        log!("swarm: MapViewOfFile {NAME} failed");
        return;
    }
    let shm = Shm(mem);
    shm.int(0).store(MAGIC, Ordering::SeqCst);
    for off in [4, 64, 72, 76, 80] {
        shm.int(off).store(0, Ordering::SeqCst);
    }
    log!("swarm: input adapter on {NAME}");
    crate::swarm_campaign::init();
    std::thread::spawn(move || Adapter::default().run(shm));
}

#[derive(Clone, Copy)]
struct Shm(*mut u8);
unsafe impl Send for Shm {}

impl Shm {
    fn int(&self, off: usize) -> &AtomicI32 {
        unsafe { AtomicI32::from_ptr(self.0.add(off) as *mut i32) }
    }

    fn float(&self, off: usize, v: f32) {
        self.int(off).store(v.to_bits() as i32, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Adapter {
    pending: Vec<String>,
    /// gameplay buttons last sent as commands
    sent: [u32; 2],
    start_held: [bool; 2],
    fire_held: [bool; 2],
    /// fire pressed since the last front end aim command
    clicked: [bool; 2],
    /// fire ignored until released (held when the front end ended)
    fire_blocked: [bool; 2],
    frontend: bool,
    frontend_sent: Option<Instant>,
    /// trace: last state log, command text written and not yet read since
    traced: Option<Instant>,
    unread: Option<Instant>,
}

/// Virtual stick axis (-32768..=32767) -> 0..1.
fn aim(v: i16) -> f32 {
    (v as i32 + 32768) as f32 / 65535.0
}

impl Adapter {
    fn run(mut self, shm: Shm) {
        let map = MAP.get().unwrap();
        loop {
            let frontend = shm.int(84).load(Ordering::SeqCst) == 1;
            if frontend != self.frontend {
                self.frontend = frontend;
                log!("swarm: {}", if frontend { "front end" } else { "gameplay" });
                for p in 0..2 {
                    self.clicked[p] = false;
                    self.fire_blocked[p] = !frontend && self.fire_held[p];
                }
            }
            if shm.int(68).swap(0, Ordering::SeqCst) == 1 {
                log!("swarm: the game asked for the adapter's test menu");
            }

            let sticks: [StickState; 2] = [wal_payload_common::input(0), wal_payload_common::input(1)];
            let mut bits = [0u32; 2];
            for p in 0..2 {
                let mut start = false;
                map.for_each_pressed(&sticks[p], |n| match n {
                    Native::Fire => bits[p] |= FIRE,
                    Native::Grenade => bits[p] |= GRENADE,
                    Native::Shield => bits[p] |= SHIELD,
                    Native::Start => start = true,
                });
                let fire = bits[p] & FIRE != 0;
                if fire && !self.fire_held[p] {
                    self.clicked[p] = true;
                }
                self.fire_held[p] = fire;
                if self.fire_blocked[p] {
                    if fire {
                        bits[p] &= !FIRE;
                    } else {
                        self.fire_blocked[p] = false;
                    }
                }
                if start && !self.start_held[p] {
                    shm.int(72).fetch_or(1 << p, Ordering::SeqCst);
                    log!("swarm: start player {}", p + 1);
                }
                self.start_held[p] = start;
                if frontend {
                    bits[p] = 0;
                }
                self.commands(p, bits[p]);
            }

            if frontend && self.frontend_sent.is_none_or(|t| t.elapsed() >= Duration::from_millis(33)) {
                self.frontend_sent = Some(Instant::now());
                self.pending.retain(|c| !c.starts_with("scriptRun UI:PcFrontendAim("));
                for p in 0..2 {
                    let fire = self.clicked[p] || self.fire_held[p];
                    self.clicked[p] = false;
                    let (x, y) = (aim(sticks[p].axis(Axis::LeftX)), aim(sticks[p].axis(Axis::LeftY)));
                    self.pending.push(format!("scriptRun UI:PcFrontendAim({p},{x:.6},{y:.6},{fire})"));
                }
            }

            shm.int(4).store(0, Ordering::SeqCst);
            shm.int(8).store(unsafe { GetTickCount() } as i32, Ordering::SeqCst);
            for p in 0..2 {
                let off = 16 + 16 * p;
                shm.float(off, aim(sticks[p].axis(Axis::LeftX)));
                shm.float(off + 4, aim(sticks[p].axis(Axis::LeftY)));
                shm.int(off + 8).store(bits[p] as i32, Ordering::SeqCst);
            }
            shm.int(4).store(1, Ordering::SeqCst);
            if trace() && self.traced.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                self.traced = Some(Instant::now());
                let aim = |p: usize| (aim(sticks[p].axis(Axis::LeftX)), aim(sticks[p].axis(Axis::LeftY)));
                log!("swarm: P1 {:.3?} {:#x}, P2 {:.3?} {:#x}", aim(0), bits[0], aim(1), bits[1]);
            }
            self.flush(shm);
            crate::swarm_campaign::poll();
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    /// Gameplay button changes -> console commands.
    fn commands(&mut self, p: usize, bits: u32) {
        let changed = self.sent[p] ^ bits;
        let sign = |b| if bits & b != 0 { "+" } else { "-" };
        if changed & FIRE != 0 {
            self.pending.push(format!("{}char_attack {p}", sign(FIRE)));
        }
        if changed & GRENADE != 0 && bits & GRENADE != 0 {
            self.pending.push(format!("char_throw {p}"));
        }
        if changed & SHIELD != 0 {
            self.pending.push(format!("{}char_block {p} 2 2", sign(SHIELD)));
        }
        self.sent[p] = bits;
    }

    /// Pending commands -> the command text, once the game has read the previous one.
    fn flush(&mut self, shm: Shm) {
        if shm.int(64).load(Ordering::SeqCst) != 0 {
            if trace() && self.unread.is_some_and(|t| t.elapsed() >= Duration::from_secs(2)) {
                log!("swarm: command text not read by the game for 2 s");
                self.unread = None;
            }
            return;
        }
        if self.pending.is_empty() {
            return;
        }
        let mut text = String::new();
        while let Some(c) = self.pending.first() {
            if text.len() + c.len() + 1 >= TEXT_MAX {
                break;
            }
            text.push_str(c);
            text.push('\n');
            self.pending.remove(0);
        }
        if text.is_empty() {
            // a command longer than the text: dropped
            self.pending.remove(0);
            return;
        }
        let bytes = text.as_bytes();
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), shm.0.add(128), bytes.len());
            *shm.0.add(128 + bytes.len()) = 0;
        }
        shm.int(64).store(1, Ordering::SeqCst);
        if trace() {
            self.unread = Some(Instant::now());
            if !text.starts_with("scriptRun UI:PcFrontendAim(") {
                log!("swarm: sent {:?}", text);
            }
        }
    }
}
