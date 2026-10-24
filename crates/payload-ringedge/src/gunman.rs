//! Medal no Gunman's satellite I/O board (`WAL_RINGEDGE_GUNMAN_BOARD=<port>`, `COM1`): the
//! two guns, their buttons and the medal mechanism, on a serial port at 38400 baud.
//!
//! JVS framing (`E0`, node, length, data, checksum; `D0` escapes `D0`/`E0` as `D0`, byte - 1)
//! with the board's own commands, several per request. Requests go to node 1; the reply (node
//! 0) is a status (1: ok), a sequence byte, then per command a report (1: ok) and its data.
//! The game's parser (gunman_dxF.exe 0x5d9370) reads, per command:
//!
//! * `60`: 5 bytes; `61`, `62`, `63`, `67`, `6D`, `70`, `73`, `75`: nothing; `66`, `68`, `69`:
//!   a big endian u16 (`66`: medals counted by the board, added to the credit at boot);
//!   `6A`, `6B`, `6C`, `71`: 3 bytes; `72`: 5 bytes (the key `74`/`73` send back);
//!   `6E`: an entry count, then the entries (none here).
//! * `64`: the guns: the high 8 bits of 4 10-bit channels (player 1 X, Y, player 2 X, Y),
//!   then their low 2 bits packed (channel 0 in bits 7-6). The game inverts channels 0, 2, 3.
//! * `65`: 3 switch bytes, active low: byte 0 bit 7 / 6 player 1 trigger / gun button, byte 1
//!   bit 1 / 0 player 2's.
//!   Inserted medals: `66` (a coin inserts `WAL_RINGEDGE_GUNMAN_MEDALS`, default 10), which
//!   the game adds to its credit.
//! * `6F` (no report): state 1, an error mask (0: none), a byte, then non-zero (else the game
//!   stops after 180 polls on "hopper" error 26).
//!
//! `74` (sent first, with the key) only needs the status. `2F` asks for the last reply again.
//!
//! Other Sega medal cabinets have the same board with their own switches ([`install`], Lady
//! Luck: `satellite.rs`).
//!
//! `WAL_RINGEDGE_GUNMAN_CREDITS=<n>`: the game's credit (its medals) kept at `n` or more
//! ([`keep_credits`]); with the profile's patches making reloads free, a free play.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};

use wal_payload_common::{input, log, serial};
use wal_protocol::{Axis, button};

/// Request length of each command, with the command byte (`6D`: plus its data length).
fn request_len(cmd: u8, rest: &[u8]) -> usize {
    match cmd {
        0x01 | 0x61 => 5,
        0x62 => 7,
        0x63 | 0x68 | 0x69 | 0x6B | 0x6C => 4,
        0x66 | 0x67 | 0x6A | 0x70 | 0x71 | 0x75 => 2,
        0x73 | 0x74 => 6,
        0x6D => 3 + rest.get(1).copied().unwrap_or(0) as usize,
        _ => 1,
    }
}

static SEQ: AtomicU8 = AtomicU8::new(0);
/// Medals inserted since the last `66`, and the coin button's last state.
static MEDALS: AtomicU16 = AtomicU16::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// The key of the last `74`/`73` (`72` returns it).
static KEY: Mutex<[u8; 5]> = Mutex::new([0x4B, 0x2D, 0x8E, 0x95, 0x00]);

/// The board's 3 switch bytes (`65`), per cabinet.
static SWITCHES: OnceLock<fn() -> [u8; 3]> = OnceLock::new();

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_RINGEDGE_GUNMAN_BOARD") else { return };
    install(port.trim(), switches);
    log!("gunman: satellite I/O board on {}", port.trim());
    keep_credits();
}

/// gunman_dxF.exe's credit manager pointer (rva): the credit is the u16 its `+4` points to
/// (the getter at 0x4e5760, the reload's subtraction 0x4e5740).
const CREDIT_MANAGER: usize = 0x68d378;

/// `WAL_RINGEDGE_GUNMAN_CREDITS`: raises the credit to `n` whenever it is below (at boot, after
/// spending), checked twice a second.
fn keep_credits() {
    let Some(n) = std::env::var("WAL_RINGEDGE_GUNMAN_CREDITS").ok().and_then(|v| v.trim().parse::<u16>().ok()) else { return };
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    log!("gunman: credit kept at {n} or more");
    std::thread::spawn(move || {
        let read = |addr: usize, out: *mut u8, len: usize| unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                addr as *const _,
                out.cast(),
                len,
                std::ptr::null_mut(),
            ) != 0
        };
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let (mut manager, mut credit, mut value) = (0usize, 0usize, 0u16);
            if !read(base + CREDIT_MANAGER, (&raw mut manager).cast(), 4) || manager == 0 {
                continue;
            }
            if !read(manager + 4, (&raw mut credit).cast(), 4) || credit == 0 || !read(credit, (&raw mut value).cast(), 2) {
                continue;
            }
            if value < n {
                unsafe { (credit as *mut u16).write_volatile(n) };
            }
        }
    });
}

/// The board on `port`, its switch bytes read by `switches`.
pub(crate) fn install(port: &str, switches: fn() -> [u8; 3]) {
    let _ = SWITCHES.set(switches);
    serial::install(port, reply);
}

/// The screen in the game's default gun calibration (its test menu's controller adjustment):
/// raw values of X left / right edges, Y top / bottom, per player. The game maps X from
/// `min - 0.4 * (max - min)` to `max` over 1360 pixels (1024 visible) and Y from
/// `min - (max - min)` to `max` over 768 (adjustment values: player 1 X 230-750, Y 580-980,
/// player 2 X 170-660, Y 450-860).
const SCREEN: [[f32; 4]; 2] = [[22.0, 570.2, 180.0, 980.0], [0.0, 496.9, 40.0, 860.0]];

/// A gun channel as the game reads it (after its inversions): the stick's position on the
/// screen of the default calibration.
fn channel(player: usize, axis: Axis) -> u16 {
    let s = input(player).axis_u16(axis) as f32 / 65535.0;
    let [x0, x1, y0, y1] = SCREEN[player];
    let (lo, hi) = if axis == Axis::LeftX { (x0, x1) } else { (y0, y1) };
    (lo + s * (hi - lo)).round().clamp(0.0, 1023.0) as u16
}

fn guns() -> [u8; 5] {
    // channels 0, 2, 3 inverted by the game
    let raw = [1023 - channel(0, Axis::LeftX), channel(0, Axis::LeftY), 1023 - channel(1, Axis::LeftX), 1023 - channel(1, Axis::LeftY)];
    let mut out = [0u8; 5];
    for (i, v) in raw.iter().enumerate() {
        out[i] = (v >> 2) as u8;
        out[4] |= ((v & 3) as u8) << (6 - 2 * i);
    }
    out
}

/// Medals per coin: `WAL_RINGEDGE_GUNMAN_MEDALS` (default 10).
fn medals_per_coin() -> u16 {
    static N: OnceLock<u16> = OnceLock::new();
    *N.get_or_init(|| std::env::var("WAL_RINGEDGE_GUNMAN_MEDALS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(10))
}

/// A coin (any player's) inserts medals in the board's medal selector.
fn poll_coin() {
    let down = (0..2).any(|p| input(p).pressed(button::COIN));
    if down && !COIN_HELD.swap(true, Ordering::Relaxed) {
        MEDALS.fetch_add(medals_per_coin(), Ordering::Relaxed);
    } else if !down {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
}

fn switches() -> [u8; 3] {
    let (p1, p2) = (input(0), input(1));
    // active high: byte 1 bit 7 front door closed, bit 6 payout button, bit 4 reset key, bit 2
    // medal selector alarm; active low: the gun buttons, byte 0 bits 5-4
    let mut s = [0xF0u8, 0x83, 0x00];
    let mut press = |byte: usize, bit: u8, on: bool| {
        if on {
            s[byte] &= !(1 << bit);
        }
    };
    press(0, 7, p1.pressed(button::B1));
    press(0, 6, p1.pressed(button::B2));
    press(1, 1, p2.pressed(button::B1));
    press(1, 0, p2.pressed(button::B2));
    // the payout button (active high): any player's start
    if p1.pressed(button::START) || p2.pressed(button::START) {
        s[1] |= 0x40;
    }
    s
}

/// The data of one command's reply (report included, except `6F`).
fn answer(cmd: u8, req: &[u8]) -> Vec<u8> {
    match cmd {
        0x60 => vec![1, 0, 0, 0, 0, 0],
        0x64 => [&[1u8][..], &guns()].concat(),
        0x65 => [&[1u8][..], &SWITCHES.get().map_or([0; 3], |f| f())].concat(),
        0x66 => {
            let n = MEDALS.swap(0, Ordering::Relaxed);
            if n != 0 {
                log!("gunman: {n} medal(s) in");
            }
            [&[1u8][..], &n.to_be_bytes()].concat()
        }
        0x68 | 0x69 => vec![1, 0, 0],
        0x6A | 0x6B | 0x6C | 0x71 => vec![1, 0, 0, 0],
        0x6E => vec![1, 0],
        0x6F => vec![1, 0, 0, 1],
        0x72 => [&[1u8][..], &*KEY.lock().unwrap()].concat(),
        0x73 | 0x74 => {
            if let Some(k) = req.get(1..6) {
                KEY.lock().unwrap().copy_from_slice(k);
            }
            vec![1]
        }
        _ => vec![1],
    }
}

fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut it = data.iter();
    while let Some(&b) = it.next() {
        out.push(if b == 0xD0 { it.next().map_or(b, |n| n.wrapping_add(1)) } else { b });
    }
    out
}

fn frame(body: &[u8]) -> Vec<u8> {
    // node 0, length (data + checksum)
    let mut packet = vec![0x00, body.len() as u8 + 1];
    packet.extend_from_slice(body);
    packet.push(packet.iter().fold(0u8, |s, b| s.wrapping_add(*b)));
    let mut out = vec![0xE0];
    for b in packet {
        if b == 0xE0 || b == 0xD0 {
            out.extend_from_slice(&[0xD0, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

fn reply(data: &[u8]) -> Vec<u8> {
    poll_coin();
    let mut out = Vec::new();
    for raw in data.split(|&b| b == 0xE0).filter(|p| !p.is_empty()) {
        let p = unescape(raw);
        // node, length, commands, checksum
        if p.len() < 3 || p[1] as usize + 2 != p.len() {
            log!("gunman: bad packet {raw:02X?}");
            continue;
        }
        let commands = &p[2..p.len() - 1];
        if commands.first() == Some(&0x2F) {
            out.extend_from_slice(&LAST.lock().unwrap());
            continue;
        }
        let mut body = vec![1, SEQ.fetch_add(1, Ordering::Relaxed).wrapping_add(1)];
        let mut i = 0;
        while i < commands.len() {
            let cmd = commands[i];
            let len = request_len(cmd, &commands[i..]).min(commands.len() - i);
            if cmd != 0x74 {
                body.extend_from_slice(&answer(cmd, &commands[i..i + len]));
            } else {
                answer(cmd, &commands[i..i + len]);
            }
            i += len;
        }
        let r = frame(&body);
        *LAST.lock().unwrap() = r.clone();
        out.extend_from_slice(&r);
    }
    out
}
