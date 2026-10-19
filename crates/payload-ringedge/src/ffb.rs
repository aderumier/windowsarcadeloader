//! The wheel's drive board (`WAL_RINGEDGE_FFB=<port>`, e.g. `COM1`): Initial D's force feedback
//! motor controller on a serial port.
//!
//! The game sends 4-byte packets: a command, two parameters, a checksum (`(command ^ p1 ^ p2)
//! & 0x7F`); the board answers each with a status byte: `0x00` ready, `0x11` not initialized
//! (after the `81` reset), `0x44` busy (the first `FC`, the wheel bounds test).
//!
//! Effects, sent to the launcher as the force feedback outputs:
//! * `84 <direction> <force>`: constant force, direction 0 to the right (force `0x7F` weakest ..
//!   `0x01` strongest), direction 1 to the left (`0x00` none .. `0x7F` strongest);
//! * `85 <period> <force>`: vibration (bumps, gravel, contacts), force 0 .. `0x7F`;
//! * `86 <..> <force>`: centering spring, force 1 .. `0x7F`; `80 00 01`: spring at full
//!   strength (`80 00 00`: none).
//!
//! `WAL_RINGEDGE_FFB_MAX=<n>`: the constant force a game sends at full strength (default 127;
//! Sonic & Sega All-Stars Racing: 20, its forces stay within `0x6C`-`0x94`).
//!
//! `WAL_RINGEDGE_FFB_SPRING=p1`: `86 <force> <..>`, the spring's force in the first parameter,
//! and `80 00 01` / `80 00 00` turn that spring on / off (Sonic & Sega All-Stars Racing: each
//! frame `86 2F 14`, `80 00 01`; the first parameter lower in races).
//!
//! `WAL_RINGEDGE_FFB=model2:<rva>` (SEGA Racing Classic, Daytona USA's Model 2 game): no board,
//! the game keeps the Model 2 drive board's command byte at `<rva>` of its image (its I/O
//! layer's output word, never sent), read every 16 ms. Commands, `n` = the force level (`0x?8`
//! strongest .. `0x?F` weakest, 8 steps):
//! * `98+n`: pushes the wheel to the right, `A8+n` to the left;
//! * `B8+n`: vibration (the road's bumps, crashes);
//! * `C0+n`: centering spring (`C0` strongest .. `C7`);
//! * `D8+n`: friction (the clutch), played as a spring;
//! anything else (`00`, the other commands) stops the effects.
//!
//! `WAL_RINGEDGE_FFB_LOG=1` logs the packets (Model 2: the command bytes, on change).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wal_payload_common::{log, send_output, serial};
use wal_protocol::output;

const READY: u8 = 0x00;
const NOT_INIT: u8 = 0x11;
const BUSY: u8 = 0x44;

struct Board {
    /// Bytes of an incomplete packet.
    pending: Vec<u8>,
    /// `FC` seen once (busy the first time).
    bounds_tested: bool,
    /// Last values sent (constant, spring, vibration).
    sent: [Option<i32>; 3],
    /// `WAL_RINGEDGE_FFB_SPRING=p1`: the last `86` force, and whether `80` turned it on.
    spring: (u8, bool),
}

static BOARD: Mutex<Board> =
    Mutex::new(Board { pending: Vec::new(), bounds_tested: false, sent: [None; 3], spring: (0, false) });
static LOG: AtomicBool = AtomicBool::new(false);
static SPRING_P1: AtomicBool = AtomicBool::new(false);
static CONSTANT_MAX: AtomicU8 = AtomicU8::new(0x7F);

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_RINGEDGE_FFB") else { return };
    LOG.store(std::env::var("WAL_RINGEDGE_FFB_LOG").is_ok_and(|v| v.trim() == "1"), Ordering::Relaxed);
    if let Some(rva) = port.trim().strip_prefix("model2:") {
        let Ok(rva) = usize::from_str_radix(rva.trim_start_matches("0x"), 16) else {
            log!("ffb: bad Model 2 command byte address {rva:?}");
            return;
        };
        let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
        log!("ffb: Model 2 drive board command byte at +{rva:#x}");
        std::thread::spawn(move || model2(base + rva));
        return;
    }
    if let Some(max) = std::env::var("WAL_RINGEDGE_FFB_MAX").ok().and_then(|v| v.trim().parse::<u8>().ok()) {
        CONSTANT_MAX.store(max.clamp(1, 0x7F), Ordering::Relaxed);
    }
    SPRING_P1.store(std::env::var("WAL_RINGEDGE_FFB_SPRING").is_ok_and(|v| v.trim() == "p1"), Ordering::Relaxed);
    serial::install(port.trim(), reply);
    log!("ffb: drive board on {}", port.trim());
}

fn reply(data: &[u8]) -> Vec<u8> {
    let mut board = BOARD.lock().unwrap();
    board.pending.extend_from_slice(data);
    let mut out = Vec::new();
    while board.pending.len() >= 4 {
        let p: [u8; 4] = board.pending[..4].try_into().unwrap();
        board.pending.drain(..4);
        if LOG.load(Ordering::Relaxed) {
            log!("ffb: {p:02X?}");
        }
        out.push(packet(&mut board, p));
    }
    out
}

/// A packet's effect, its status byte.
fn packet(board: &mut Board, [command, p1, p2, _]: [u8; 4]) -> u8 {
    let scale = |v: u8| (v.min(0x7F) as i32 * output::FFB_MAX) / 0x7F;
    let max = CONSTANT_MAX.load(Ordering::Relaxed);
    let constant = |v: u8| (v.min(max) as i32 * output::FFB_MAX) / max as i32;
    match command {
        0x81 => return NOT_INIT,
        0xFC if !board.bounds_tested => {
            board.bounds_tested = true;
            return BUSY;
        }
        0x84 => {
            let force = match p1 {
                0 if p2 > 0 => constant(0x80 - p2.min(0x80)),
                1 => -constant(p2),
                _ => 0,
            };
            send(board, 0, output::FFB_CONSTANT, force);
        }
        0x85 => send(board, 2, output::FFB_VIBRATION, if p1 > 0 { scale(p2) } else { 0 }),
        0x86 | 0x80 if SPRING_P1.load(Ordering::Relaxed) => {
            if command == 0x86 {
                board.spring.0 = p1;
            } else {
                board.spring.1 = p2 == 1;
            }
            let (force, on) = board.spring;
            send(board, 1, output::FFB_SPRING, if on { scale(force) } else { 0 });
        }
        0x86 if (1..0x80).contains(&p2) => send(board, 1, output::FFB_SPRING, scale(p2)),
        0x80 => send(board, 1, output::FFB_SPRING, if p2 == 1 { output::FFB_MAX } else { 0 }),
        _ => {}
    }
    READY
}

/// Sends an output on change.
fn send(board: &mut Board, slot: usize, id: u16, value: i32) {
    if board.sent[slot] != Some(value) {
        if board.sent[slot].is_none() {
            log!("ffb: first output {id} = {value}");
        }
        board.sent[slot] = Some(value);
        send_output(0, id, value);
    }
}

/// `WAL_RINGEDGE_FFB=model2:<rva>`: polls the Model 2 command byte at `address`, sends its
/// effects on change (and every second, for a launcher connecting late).
fn model2(address: usize) {
    let mut board = Board { pending: Vec::new(), bounds_tested: false, sent: [None; 3], spring: (0, false) };
    let mut last = None;
    let mut refreshed = std::time::Instant::now();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(16));
        // the game's .data, mapped for the process lifetime
        let command = unsafe { std::ptr::read_volatile(address as *const u8) };
        if LOG.load(Ordering::Relaxed) && last != Some(command) {
            log!("ffb: {command:02X}");
        }
        last = Some(command);
        // 8 levels, the lowest command the strongest
        let level = |low: u8| ((low + 8 - command) as i32 * output::FFB_MAX) / 8;
        let effects = match command {
            0x98..=0x9F => [level(0x98), 0, 0],
            0xA8..=0xAF => [-level(0xA8), 0, 0],
            0xB8..=0xBF => [0, 0, level(0xB8)],
            0xC0..=0xC7 | 0xD8..=0xDF => [0, level(command & 0xF8), 0],
            _ => [0, 0, 0],
        };
        if refreshed.elapsed().as_secs() >= 1 {
            // sent again
            board.sent = [Some(i32::MIN); 3];
            refreshed = std::time::Instant::now();
        }
        for (slot, (id, value)) in [output::FFB_CONSTANT, output::FFB_SPRING, output::FFB_VIBRATION].into_iter().zip(effects).enumerate() {
            send(&mut board, slot, id, value);
        }
    }
}
