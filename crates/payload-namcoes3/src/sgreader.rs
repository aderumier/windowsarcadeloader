//! Sega's Aime NFC reader (837-15396, the reader of Bandai Namco's BNA1 cabinets, through Sega's
//! `libamw.dll`) on a serial port, ported from bananatools' `board/sg-frame.c`, `sg-cmd.c`,
//! `sg-nfc.c` and `sg-led.c` (public domain).
//!
//! Frames: `E0`, then the escaped bytes (`D0 x` is `x + 1`) of `len addr seq cmd ...` and a
//! checksum (the sum of the unescaped bytes from `len`). A request's body is a payload length
//! and the payload; an answer's a status and a payload length before it. Two units on the line:
//! the NFC reader (address 0) and its LED board (address 8).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use wal_payload_common::log;

/// The card on the reader (an Aime card's access code, 10 BCD bytes), until when.
static CARD: Mutex<Option<([u8; 10], Instant)>> = Mutex::new(None);

/// Puts the card of `access_code` (20 digits) on the reader for `time`.
pub(crate) fn insert(access_code: &str, time: Duration) {
    let d = access_code.as_bytes();
    if d.len() != 20 || !d.iter().all(u8::is_ascii_digit) {
        return log!("sgreader: access code {access_code:?} is not 20 digits");
    }
    let mut bcd = [0u8; 10];
    for (b, pair) in bcd.iter_mut().zip(d.chunks(2)) {
        *b = (pair[0] - b'0') << 4 | (pair[1] - b'0');
    }
    *CARD.lock().unwrap() = Some((bcd, Instant::now() + time));
}

fn card() -> Option<[u8; 10]> {
    CARD.lock().unwrap().filter(|(_, until)| Instant::now() < *until).map(|(c, _)| c)
}

/// An Aime card's MIFARE blocks 0..=3 (bananatools' `iccard/aime.c`): the UID (its first 4
/// bytes), `SBSD`, the access code at block 2's byte 6.
fn block(code: &[u8; 10], n: u8) -> [u8; 16] {
    let mut b = [0u8; 16];
    match n {
        0 => b[..4].copy_from_slice(&code[..4]),
        1 => b[..4].copy_from_slice(b"SBSD"),
        2 => b[6..].copy_from_slice(code),
        _ => {}
    }
    b
}

/// The reader (generation 3): hardware and firmware versions, the LED board's info.
const HW_VERSION: &[u8] = b"837-15396";
const FW_VERSION: &[u8] = b"\x94";
const LED_INFO: &[u8] = b"000-00000\xFF\x11\x40";

const NFC: u8 = 0x00;
const LED: u8 = 0x08;

/// The unescaped frame (`len` .. the last body byte), its checksum checked.
fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let (&sync, rest) = bytes.split_first()?;
    if sync != 0xE0 {
        return None;
    }
    let mut out = Vec::with_capacity(rest.len());
    let mut it = rest.iter();
    while let Some(&b) = it.next() {
        match b {
            0xE0 => return None,
            0xD0 => out.push(it.next()?.wrapping_add(1)),
            b => out.push(b),
        }
    }
    let checksum = out.pop()?;
    if out.first().map(|&n| n as usize) != Some(out.len()) || out.iter().fold(0u8, |s, b| s.wrapping_add(*b)) != checksum {
        return None;
    }
    Some(out)
}

fn encode(frame: &[u8]) -> Vec<u8> {
    let mut out = vec![0xE0];
    let checksum = frame.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    for &b in frame.iter().chain(Some(&checksum)) {
        if b == 0xD0 || b == 0xE0 {
            out.extend([0xD0, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

/// An answer to the request `req` (`len addr seq cmd`): status, payload.
fn answer(req: &[u8], status: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![6 + payload.len() as u8, req[1], req[2], req[3], status, payload.len() as u8];
    frame.extend_from_slice(payload);
    encode(&frame)
}

fn nfc(req: &[u8]) -> Option<Vec<u8>> {
    let cmd = req[3];
    let payload = &req[5..];
    Some(match cmd {
        // reset: status 3
        0x62 => answer(req, 3, &[]),
        0x30 => answer(req, 0, FW_VERSION),
        0x32 => answer(req, 0, HW_VERSION),
        // poll: a MIFARE target (type 0x10, its 4-byte UID) or none
        0x42 => match card() {
            Some(code) => answer(req, 0, &[1, 0x10, 4, code[0], code[1], code[2], code[3]]),
            None => answer(req, 0, &[0]),
        },
        // read block (UID, block number)
        0x52 => match (card(), payload.get(4)) {
            (Some(code), Some(&n)) => {
                // the access code read: the card leaves the reader (a touch)
                if n == 2 {
                    *CARD.lock().unwrap() = None;
                    log!("sgreader: card read");
                }
                answer(req, 0, &block(&code, n))
            }
            _ => answer(req, 1, &[]),
        },
        // authenticate, firmware data (0x2b bytes: "the firmware is identical")
        0x51 | 0x55 => answer(req, 0, &[]),
        0x61 => answer(req, if payload.len() == 0x2b { 0x20 } else { 0 }, &[]),
        // select, keys, radio on / off, update mode
        0x40 | 0x41 | 0x43 | 0x50 | 0x54 | 0x60 => answer(req, 0, &[]),
        _ => {
            log!("sgreader: nfc command {cmd:#04x} not handled ({payload:02X?})");
            answer(req, 1, &[])
        }
    })
}

fn led(req: &[u8]) -> Option<Vec<u8>> {
    match req[3] {
        0xF5 => Some(answer(req, 0, &[0])),
        0xF0 => Some(answer(req, 0, LED_INFO)),
        // set color: no answer
        0x81 => None,
        cmd => {
            log!("sgreader: led command {cmd:#04x} not handled");
            Some(answer(req, 1, &[]))
        }
    }
}

/// The reader's answers to the frames written in one call.
fn handle(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // a write can hold several frames
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start + 1..].iter().position(|&b| b == 0xE0).map_or(bytes.len(), |p| start + 1 + p);
        match decode(&bytes[start..end]) {
            Some(req) if req.len() >= 5 && req[4] as usize == req.len() - 5 => {
                let reply = match req[1] {
                    NFC => nfc(&req),
                    LED => led(&req),
                    _ => None,
                };
                out.extend(reply.unwrap_or_default());
            }
            _ => log!("sgreader: bad frame {:02X?}", &bytes[start..end]),
        }
        start = end;
    }
    out
}

/// The reader on `port`, for the serial calls of `modules`.
pub(crate) fn init(port: &str, modules: &[usize]) {
    wal_payload_common::serial::install_in(port, handle, modules);
    log!("sgreader: Aime reader on {port}");
}
