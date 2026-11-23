//! Cabinet outputs by their MAME names (lamps, gun recoil, the drive board's bytes), for the
//! launcher's network outputs (`WAL_OUTPUTS`: MAMEHooker, BackForceFeeder): each value sent to
//! the launcher when it changes ([`Message::Named`]), every known one again when the payload
//! (re)connects. The names are those MAME and its tools use:
//!
//! * `RawLamps`, `RawLamps1`... (a JVS board's general purpose output bytes) and `lamp0`,
//!   `lamp1`... (each output, lamp n bit 7 - n % 8 of byte n / 8): [`gpo`], [`lamps`]
//! * `RawDrive`: each byte the game sends to a Sega drive board, in order ([`drive`])
//! * the games' own names (`P1_LmpStart`, `P1_GunRecoil`...): [`set`], [`pulse`]
//!
//! Nothing is kept or sent with the option off.

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wal_protocol::Message;

use crate::log;

/// Known values, in the order first set (sent again on reconnection).
static VALUES: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());

/// The launcher's network outputs are on (`WAL_OUTPUTS=1`).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("WAL_OUTPUTS").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "on"));
        if on {
            log!("outputs: network outputs on");
        }
        on
    })
}

fn send(name: &str, value: i32) {
    if let Some(stream) = crate::WRITER.lock().unwrap().as_mut() {
        let _ = stream.write_all(&Message::Named { name: name.to_string(), value }.encode());
    }
}

/// Sets output `name` to `value`, sent when it changes.
pub fn set(name: &str, value: i32) {
    if !enabled() {
        return;
    }
    let mut values = VALUES.lock().unwrap();
    match values.iter_mut().find(|(n, _)| n == name) {
        Some((_, v)) if *v == value => return,
        Some((_, v)) => *v = value,
        None => values.push((name.to_string(), value)),
    }
    send(name, value);
}

/// Output `name` at 1 for `ms` milliseconds, then 0 (a gun's recoil solenoid, a short flash):
/// a pulse still on is not restarted.
pub fn pulse(name: &'static str, ms: u64) {
    if !enabled() || VALUES.lock().unwrap().iter().any(|(n, v)| n == name && *v != 0) {
        return;
    }
    set(name, 1);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        set(name, 0);
    });
}

/// General purpose output byte `index` of a JVS board (byte 0: outputs 1 to 8 in bits 7 to 0):
/// `RawLamps<index>` and each changed `lamp<n>`.
pub fn gpo(index: usize, byte: u8) {
    if !enabled() {
        return;
    }
    let raw = if index == 0 { "RawLamps".to_string() } else { format!("RawLamps{index}") };
    let old = VALUES.lock().unwrap().iter().find(|(n, _)| *n == raw).map(|(_, v)| *v as u8);
    if old == Some(byte) {
        return;
    }
    set(&raw, byte as i32);
    let changed = old.map_or(0xFF, |o| o ^ byte);
    for bit in (0..8).rev().filter(|b| changed & (1 << b) != 0) {
        set(&format!("lamp{}", index * 8 + 7 - bit), (byte >> bit & 1) as i32);
    }
}

/// A JVS board's general purpose output bytes, from byte 0 (a `32` command's data).
pub fn gpo_bytes(bytes: &[u8]) {
    for (i, b) in bytes.iter().enumerate() {
        gpo(i, *b);
    }
}

/// Lamps of a board that is not a JVS one, sent as a JVS board's: lamp n is bit n of `bits`,
/// `count` lamps.
pub fn lamps(bits: u64, count: usize) {
    let mut bytes = vec![0u8; count.div_ceil(8)];
    for n in (0..count).filter(|n| bits >> n & 1 != 0) {
        bytes[n / 8] |= 0x80 >> (n % 8);
    }
    gpo_bytes(&bytes);
}

/// Bytes the game sends to a Sega drive board, in order: `RawDrive`, each of them (repeated
/// values too: the frames are the bytes' sequence).
pub fn drive(bytes: &[u8]) {
    if !enabled() {
        return;
    }
    let mut values = VALUES.lock().unwrap();
    for b in bytes {
        match values.iter_mut().find(|(n, _)| n == "RawDrive") {
            Some((_, v)) => *v = *b as i32,
            None => values.push(("RawDrive".to_string(), *b as i32)),
        }
        send("RawDrive", *b as i32);
    }
}

/// Every known value to the launcher, at the connection (values set before it are not lost).
pub(crate) fn resend() {
    if !enabled() {
        return;
    }
    let values = VALUES.lock().unwrap().clone();
    for (name, value) in values {
        send(&name, value);
    }
}

/// Reading the game's memory for its outputs (the values DemulShooter reads, at the known
/// builds' addresses): every read checked first, 0 when the address is not readable.
#[cfg(windows)]
pub mod mem {
    /// The game executable's image base.
    pub fn base() -> usize {
        unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize }
    }

    pub fn u8(addr: usize) -> u8 {
        if crate::crash::readable(addr, 1) { unsafe { std::ptr::read_volatile(addr as *const u8) } } else { 0 }
    }

    pub fn u32(addr: usize) -> u32 {
        if crate::crash::readable(addr, 4) { unsafe { std::ptr::read_unaligned(addr as *const u32) } } else { 0 }
    }

    pub fn f32(addr: usize) -> f32 {
        f32::from_bits(u32(addr))
    }

    /// The pointer at `addr`, then the pointer at each `+ offset` in turn (DemulShooter's
    /// `ReadPtrChain`); 0 at the first null pointer.
    pub fn chain(addr: usize, offsets: &[usize]) -> u32 {
        let mut p = u32(addr);
        for o in offsets {
            if p == 0 {
                return 0;
            }
            p = u32(p as usize + o);
        }
        p
    }
}

/// Runs `step` every `ms` milliseconds on a thread of its own while the outputs are on (the
/// games whose outputs are read from their memory).
pub fn poll(name: &'static str, ms: u64, mut step: impl FnMut() + Send + 'static) {
    if !enabled() {
        return;
    }
    log!("outputs: {name} read every {ms} ms");
    std::thread::spawn(move || {
        loop {
            step();
            std::thread::sleep(Duration::from_millis(ms));
        }
    });
}

/// A value that pulses an output when it goes down (DemulShooter's custom recoil on the ammo
/// count, damage on the life): `update` with the new value, `None` while it is not followed.
#[derive(Default)]
pub struct Falling {
    last: Option<i64>,
}

impl Falling {
    pub fn update(&mut self, value: Option<i64>, name: &'static str) {
        if let (Some(v), Some(last)) = (value, self.last)
            && v < last
        {
            pulse(name, 50);
        }
        self.last = value;
    }
}
