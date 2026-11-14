//! Sega's 837-15345 RFID deck reader (KanColle Arcade, Fate/Grand Order Arcade): the cards
//! laid on the cabinet's deck, read by their RFID chips (segatools' `fgohook/deck.c`, public
//! domain: OLEG, Coburn, Mitsuhide).
//!
//! Frames: `E0 cmd len payload... sum` (sum of the bytes after `E0`; `E0` / `D0` in the rest
//! escaped as `D0 byte-1`); an answer has a status byte after the command. A read command
//! answers `81` (start); then each of the game's reads of the port gets the next frame: one
//! `82` per card (its 44 bytes: the chip's 12-byte id, its 32 bytes of data), then `83` (end).
//!
//! The cards on the deck: `WAL_DECK_CARDS`, a folder of 44-byte card files (`<id>.bin`: the
//! cards [`crate::chcusb`] printed, default `printer`), at most 36.

use std::sync::Mutex;

use crate::{log, serial};

const SYNC: u8 = 0xE0;
const ESC: u8 = 0xD0;
const MAX_CARDS: usize = 36;

static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// The read in progress: the cards read at its start, the next card (then the end).
static READ: Mutex<Option<(Vec<Vec<u8>>, usize)>> = Mutex::new(None);

fn encode(cmd: u8, status: u8, payload: &[u8]) -> Vec<u8> {
    let mut plain = vec![cmd, status, payload.len() as u8];
    plain.extend_from_slice(payload);
    let sum = plain.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    plain.push(sum);
    let mut out = vec![SYNC];
    for b in plain {
        if b == SYNC || b == ESC {
            out.extend([ESC, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

/// The next whole request (`E0 cmd len payload sum`, unescaped), removed from `rx`.
fn next_frame(rx: &mut Vec<u8>) -> Option<Vec<u8>> {
    let start = rx.iter().position(|&b| b == SYNC)?;
    rx.drain(..start);
    let mut frame = vec![SYNC];
    let mut escape = false;
    for i in 1..rx.len() {
        let b = rx[i];
        if b == SYNC {
            rx.drain(..i);
            return next_frame(rx);
        }
        if b == ESC {
            escape = true;
            continue;
        }
        frame.push(if escape { b.wrapping_add(1) } else { b });
        escape = false;
        if frame.len() >= 3 && frame.len() == frame[2] as usize + 4 {
            rx.drain(..=i);
            return Some(frame);
        }
    }
    None
}

/// The cards on the deck (44 bytes each).
fn cards() -> Vec<Vec<u8>> {
    let dir = std::env::var("WAL_DECK_CARDS").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| "printer".into());
    let mut files: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bin"))).collect();
    files.sort();
    files.iter().filter_map(|p| std::fs::read(p).ok()).filter(|d| d.len() == 44).take(MAX_CARDS).collect()
}

fn answer(frame: &[u8]) -> Vec<u8> {
    let cmd = frame[1];
    match cmd {
        // reset, initialization
        0x41 | 0x81 | 0x04 | 0x05 => encode(cmd, 0, &[]),
        // boot / application firmware versions (the dump's 837-15345 firmware: 0x91)
        0x84 => encode(cmd, 0, &[0x90]),
        0x42 => encode(cmd, 0, &[0x91]),
        0x85 => encode(cmd, 0, b"837-15345"),
        // read the deck: its start (the cards and the end follow, one per read of the port)
        0x06 => {
            let cards = cards();
            log!("deck: read, {} card(s)", cards.len());
            *READ.lock().unwrap_or_else(|e| e.into_inner()) = Some((cards, 0));
            encode(cmd, 0x81, &[])
        }
        _ => {
            log!("deck: command {cmd:#04x}");
            encode(cmd, 0, &[])
        }
    }
}

/// The read in progress' next frame, when the game reads the port: a card, then the end.
fn next_read_frame() -> Vec<u8> {
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    let Some((cards, next)) = read.as_mut() else { return Vec::new() };
    if let Some(card) = cards.get(*next) {
        *next += 1;
        return encode(0x06, 0x82, card);
    }
    *read = None;
    encode(0x06, 0x83, &[])
}

fn packet(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap_or_else(|e| e.into_inner());
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    while let Some(frame) = next_frame(&mut rx) {
        out.extend(answer(&frame));
    }
    out
}

/// The deck reader on `port`.
pub fn init(port: &str) {
    serial::install_streaming(port, packet, next_read_frame);
    log!("deck: 837-15345 deck reader on {port}");
}
