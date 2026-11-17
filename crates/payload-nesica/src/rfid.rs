//! NESiCA card reader emulation.
//!
//! The Taito RFID board sits on a serial port (`COM2`) and speaks JVS framing; the port is
//! emulated by `wal_payload_common::serial`.
//!
//! The card is inserted/removed by the virtual stick `card` input of any player (toggle).
//!
//! Options: `WAL_NESICA_RFID=0` disables it, `WAL_NESICA_RFID_PORT` (default `COM2`),
//! `WAL_NESICA_CARD_ID` (16 digits, default `7020392010281502`), `WAL_NESICA_CARD_AUTO=1`
//! inserts the card when a NESiCAxLive 2 game asks for one (no card input on a gamepad),
//! `WAL_NESICA_RFID_MODULES`: other modules opening the port, `a.dll,b.dll` (Left 4 Dead:
//! Survivors: the Source engine's `inputsystem.dll`, which loads this DLL; default: the
//! executable only).
//!
//! Type X3 games (Left 4 Dead: Survivors) read a 16-byte key from `C:\TypeX3Temp.dat` and,
//! between polls, send the board a challenge on node `0x80 | n` (`WAL_NESICA_RFID_SECURITY=1`:
//! the key file is created when missing, zeros, and the challenges are answered with it;
//! `WAL_NESICA_RFID_KEY_FILE`: another key file, Groove Coaster 2's `C:\TypeXZEROTemp.dat`). Their
//! reader's commands differ too (same option): `32` drives its lamp (a report only), `26` is the
//! card presence (one byte), `61` reads the card (8 header bytes, the 16-character id); with
//! `WAL_NESICA_CARD_AUTO=1` the card is always on the reader (no card input needed).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wal_payload_common::jvs::{self, Encoder, REPORT_OK};
use wal_payload_common::{log, serial};
use wal_protocol::button;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
const BOARD_ID: &[u8] = b"TAITO CORP.;RFID CTRL P.C.B.;Ver1.00;";
const CARD_HEADER: [u8; 8] = [0x04, 0xC2, 0x3D, 0xDA, 0x6F, 0x52, 0x80, 0x00];

static INSERTED: AtomicBool = AtomicBool::new(false);
/// The Type X3 reader's commands (`WAL_NESICA_RFID_SECURITY=1`).
static TYPEX3: AtomicBool = AtomicBool::new(false);
/// The card always on the Type X3 reader (`WAL_NESICA_CARD_AUTO=1`).
static ALWAYS: AtomicBool = AtomicBool::new(false);

fn inserted() -> bool {
    ALWAYS.load(Ordering::Relaxed) || INSERTED.load(Ordering::Relaxed)
}
static CARD: OnceLock<[u8; 0x18]> = OnceLock::new();

fn card_data() -> &'static [u8; 0x18] {
    CARD.get_or_init(|| {
        let id = std::env::var("WAL_NESICA_CARD_ID").unwrap_or_else(|_| "7020392010281502".into());
        let mut data = [b'0'; 0x18];
        data[..8].copy_from_slice(&CARD_HEADER);
        for (d, c) in data[8..].iter_mut().zip(id.bytes()) {
            *d = c;
        }
        data
    })
}

/// Answers one request packet of the game.
fn process(packet: &[u8]) -> Vec<u8> {
    let Some(req) = jvs::parse(packet) else { return Vec::new() };
    if (0x80..=0x9F).contains(&req.node) {
        return security(req.node, &req.commands);
    }
    if !matches!(req.node, 0x00 | 0x01 | 0xFF) {
        return Vec::new();
    }
    let arg = |c: &[u8], i: usize| c.get(i).copied().unwrap_or(0);
    let mut r = Encoder::new();
    let mut cmds = &req.commands[..];
    let typex3 = TYPEX3.load(Ordering::Relaxed);
    while !cmds.is_empty() {
        let step = match cmds[0] {
            // Type X3: the presence (one argument), the lamp, the card read, the other outputs
            0x26 if typex3 => {
                r.extend(&[REPORT_OK, if inserted() { 0x19 } else { 0 }]);
                2
            }
            0x32 if typex3 => {
                r.push(REPORT_OK);
                2 + arg(cmds, 1) as usize
            }
            0x61 if typex3 => {
                static READS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if READS.fetch_add(1, Ordering::Relaxed) < 8 {
                    log!("rfid: card read ({})", if inserted() { "card" } else { "no card" });
                }
                r.push(REPORT_OK);
                if inserted() {
                    r.extend(card_data());
                } else {
                    r.extend(&[0; 0x18]);
                }
                1
            }
            0x62..=0x64 if typex3 => {
                r.push(REPORT_OK);
                cmds.len()
            }
            // bus reset: the node loses its address, the sense line (CTS) drops until the next
            // F1 (Gunslinger Stratos 2: two clients, NESYS then the game's own reader thread,
            // each resets and addresses the bus; the second one waited for CTS to drop)
            0xF0 => {
                serial::READY.store(false, Ordering::Relaxed);
                2
            }
            0xF1 => {
                r.push(REPORT_OK);
                serial::READY.store(true, Ordering::Relaxed);
                2
            }
            0x01 | 0x03 => {
                r.extend(&[REPORT_OK, 1]);
                2
            }
            0x04 => {
                r.push(REPORT_OK);
                1
            }
            0x05 => {
                r.push(REPORT_OK);
                3
            }
            0x10 => {
                r.push(REPORT_OK);
                r.extend(BOARD_ID);
                r.push(0);
                1
            }
            0x11 => {
                r.extend(&[REPORT_OK, 0x13]); // command format revision
                1
            }
            0x12 => {
                r.extend(&[REPORT_OK, 0x30]); // JVS revision
                1
            }
            0x13 => {
                r.extend(&[REPORT_OK, 0x10]); // communication revision
                1
            }
            0x14 => {
                r.extend(&[REPORT_OK, 1, 7, 0, 8, 0, 0x12, 8, 0, 0, 0]); // features
                1
            }
            0x20 => {
                r.extend(&[REPORT_OK, 0, 0, 0, 0, 0]); // switches
                3
            }
            0x21 => {
                r.extend(&[REPORT_OK, 0, 0, 0, 0]); // coins
                2
            }
            0x26 => {
                // general purpose input: card presence
                let n = arg(cmds, 1) as usize;
                r.push(REPORT_OK);
                let v = if INSERTED.load(Ordering::Relaxed) { 0x19 } else { 0 };
                for _ in 0..n {
                    r.push(v);
                }
                2 + n
            }
            0x2F => 1, // retransmit
            0x30 | 0x31 => {
                r.push(REPORT_OK); // coin decrease / payout
                4
            }
            0x32 => {
                // general purpose output: card data
                let n = arg(cmds, 1) as usize;
                r.push(REPORT_OK);
                if INSERTED.load(Ordering::Relaxed) {
                    r.extend(card_data());
                } else {
                    for _ in 0..n * 0x18 {
                        r.push(0);
                    }
                }
                r.push(REPORT_OK);
                2 + n
            }
            other => {
                log!("rfid: unknown command {other:#04x}");
                r.push(REPORT_OK);
                1
            }
        };
        cmds = &cmds[step.min(cmds.len())..];
    }
    r.finish()
}

/// The Type X3 key (`C:\TypeX3Temp.dat`, `WAL_NESICA_RFID_KEY_FILE`), created with zeros when
/// missing.
fn security_key() -> &'static [u8; 16] {
    static KEY: OnceLock<[u8; 16]> = OnceLock::new();
    KEY.get_or_init(|| {
        let path = std::env::var("WAL_NESICA_RFID_KEY_FILE").unwrap_or_else(|_| "C:\\TypeX3Temp.dat".into());
        match std::fs::read(&path) {
            Ok(d) if d.len() >= 16 => d[..16].try_into().unwrap(),
            _ => {
                if let Err(e) = std::fs::write(&path, [0u8; 16]) {
                    log!("rfid: {path} not written: {e}");
                }
                [0; 16]
            }
        }
    })
}

/// Answers a Type X3 challenge (inputsystem.dll's SecInterval). The game sends on node
/// `0x80 | n`: for node 0x80 `[flag, 2 bytes, c, i]`, else `m` filler bytes (`m` = 1 + bit 0
/// of `(n >> 4) ^ key[n & 15]`), a checksum, 2 bytes, `c`, `i`. The reply is a word and the
/// seed of an LCG (`x * 0x2253 + 0x11D7`, `>> 11`): the word XORed with the LCG's second
/// output must have `key[i & 15]` as its low byte (its high byte comes back in the next
/// challenge).
fn security(node: u8, data: &[u8]) -> Vec<u8> {
    let key = security_key();
    let at = if node == 0x80 { 4 } else { 1 + (((node >> 4) ^ key[(node & 15) as usize]) & 1) as usize + 4 };
    let Some(&index) = data.get(at) else {
        log!("rfid: short challenge on node {node:#04x}: {data:02X?}");
        return Vec::new();
    };
    let seed: u16 = 0x1234;
    let mut state = u32::from(seed);
    for _ in 0..2 {
        state = state.wrapping_mul(0x2253).wrapping_add(0x11D7);
    }
    let word = u16::from(key[(index & 15) as usize]) ^ (state >> 11) as u16;
    let mut r = Encoder::new();
    r.push(REPORT_OK);
    r.extend(&word.to_le_bytes());
    r.extend(&seed.to_le_bytes());
    r.finish()
}

/// The `card` virtual input's card is in the reader (for the NESiCAxLive 2 API).
pub(crate) fn card_inserted() -> bool {
    INSERTED.load(Ordering::Relaxed)
}

/// A NESiCAxLive 2 game starts a card read: `WAL_NESICA_CARD_AUTO=1` inserts the card.
pub(crate) fn card_requested() {
    if std::env::var("WAL_NESICA_CARD_AUTO").is_ok_and(|v| v == "1") && !INSERTED.swap(true, Ordering::Relaxed) {
        log!("rfid: card inserted (auto)");
    }
}

/// The card's 16-character id.
pub(crate) fn card_id() -> String {
    std::env::var("WAL_NESICA_CARD_ID").unwrap_or_else(|_| "7020392010281502".into())
}

pub(crate) fn init() {
    if std::env::var("WAL_NESICA_RFID").is_ok_and(|v| v == "0") {
        return;
    }
    if std::env::var("WAL_NESICA_RFID_SECURITY").is_ok_and(|v| v == "1") {
        security_key();
        TYPEX3.store(true, Ordering::Relaxed);
        if std::env::var("WAL_NESICA_CARD_AUTO").is_ok_and(|v| v == "1") {
            ALWAYS.store(true, Ordering::Relaxed);
            log!("rfid: the card always on the reader");
        }
    }
    let port = std::env::var("WAL_NESICA_RFID_PORT").unwrap_or_else(|_| "COM2".into());
    let mut modules = vec![unsafe { GetModuleHandleW(std::ptr::null()) } as usize];
    for name in std::env::var("WAL_NESICA_RFID_MODULES").unwrap_or_default().split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        match unsafe { GetModuleHandleW(wide.as_ptr()) } as usize {
            0 => log!("rfid: {name} not loaded"),
            m => modules.push(m),
        }
    }
    serial::install_in(&port, process, &modules);

    // card insert/remove: toggled by the `card` virtual input
    std::thread::spawn(|| {
        let mut held = false;
        loop {
            let pressed = (0..wal_protocol::MAX_PLAYERS).any(|p| wal_payload_common::input(p).pressed(button::CARD));
            if pressed && !held {
                let now = !INSERTED.fetch_xor(true, Ordering::Relaxed);
                log!("rfid: card {}", if now { "inserted" } else { "removed" });
            }
            held = pressed;
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}
