//! The Aime card reader (`Aime_*`): player 1's `card` input puts the card on the reader.
//!
//! The game's card flow (Under Night's `AMU::PlayState`): `Aime_start` → while waiting for a card
//! `Aime_isBusy` stays true (`Aime_cancel` stops waiting: guest play) → a card read: not busy,
//! `Aime_hasResult`, `Aime_getAccessCode` (10 BCD bytes then the 20 digits as UTF-16), then
//! `Aime_getAimeId`; the game then loads the player's data from the title server (`apms.rs`).
//!
//! Options: `WAL_APM3_CARD_AUTO=1` (the card always on the reader: read at every prompt, the
//! `card` input takes it off), `WAL_APM3_ACCESS_CODE` (20 digits, default
//! `50000000000000000001`), `WAL_APM3_AIME_ID` (default: from the access code). The reader needs
//! the Aime server (`WAL_APM3_ONLINE=1`).

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use wal_payload_common::log;
use wal_protocol::button;

use crate::{ONLINE, env, first_call};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reader {
    Idle,
    /// Waiting for a card (the card input released first: one read per press).
    Waiting { released: bool },
    /// A card was read.
    Done,
}

static READER: Mutex<Reader> = Mutex::new(Reader::Idle);

fn access_code() -> String {
    let code = env("WAL_APM3_ACCESS_CODE").unwrap_or_else(|| "50000000000000000001".into());
    let digits: String = code.chars().filter(char::is_ascii_digit).collect();
    if digits.len() == 20 { digits } else { "50000000000000000001".into() }
}

fn aime_id() -> u32 {
    env("WAL_APM3_AIME_ID").and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        // a stable id from the code: its last 9 digits (an Aime id is a positive 32-bit number)
        let code = access_code();
        code[11..].parse::<u32>().unwrap_or(1).max(1)
    })
}

/// The card is on the reader: the `card` input toggled on, or (`WAL_APM3_CARD_AUTO=1`) always
/// unless toggled off.
fn card_on_reader() -> bool {
    static AUTO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let auto = *AUTO.get_or_init(|| env("WAL_APM3_CARD_AUTO").is_some_and(|v| v == "1"));
    wal_payload_common::input(0).pressed(button::CARD) != auto
}

/// The reader's state, advanced: a waiting reader reads the card once it is put on it.
fn poll() -> Reader {
    let mut reader = READER.lock().unwrap();
    if let Reader::Waiting { released } = *reader {
        let on = card_on_reader();
        if !on {
            *reader = Reader::Waiting { released: true };
        } else if released {
            log!("aime: card read, access code {} aime id {}", access_code(), aime_id());
            *reader = Reader::Done;
        }
    }
    *reader
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_start() -> bool {
    first_call("Aime_start");
    if !ONLINE.load(Ordering::Relaxed) {
        return false;
    }
    log!("aime: waiting for a card");
    // a card already held is read too (the player pressed the card input before the prompt)
    *READER.lock().unwrap() = Reader::Waiting { released: true };
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_cancel() -> bool {
    first_call("Aime_cancel");
    let mut reader = READER.lock().unwrap();
    if *reader != Reader::Idle {
        log!("aime: cancelled");
    }
    *reader = Reader::Idle;
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isBusy() -> bool {
    first_call("Aime_isBusy");
    matches!(poll(), Reader::Waiting { .. })
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_hasResult() -> bool {
    first_call("Aime_hasResult");
    poll() == Reader::Done
}

/// `AccessCode`: 10 bytes (the 20 digits as BCD), then the digits as a NUL terminated UTF-16
/// string (21 characters).
#[unsafe(no_mangle)]
pub extern "system" fn Aime_getAccessCode(out: *mut u8) -> bool {
    first_call("Aime_getAccessCode");
    if out.is_null() || poll() != Reader::Done {
        return false;
    }
    let code = access_code();
    let digits: Vec<u8> = code.bytes().map(|b| b - b'0').collect();
    let mut data = [0u8; 0x34];
    for (i, pair) in digits.chunks(2).enumerate() {
        data[i] = pair[0] << 4 | pair[1];
    }
    for (i, c) in code.encode_utf16().enumerate() {
        data[10 + 2 * i..12 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len()) };
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_getAimeId(out: *mut u32) -> bool {
    first_call("Aime_getAimeId");
    let read = poll() == Reader::Done;
    if !out.is_null() {
        unsafe { *out = if read { aime_id() } else { u32::MAX } };
    }
    read
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_acceptConfirm() -> bool {
    first_call("Aime_acceptConfirm");
    false
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_getConfirm() -> u32 {
    first_call("Aime_getConfirm");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_hasConfirm() -> bool {
    first_call("Aime_hasConfirm");
    false
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_getErrorCategory() -> u32 {
    first_call("Aime_getErrorCategory");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_hasError() -> bool {
    first_call("Aime_hasError");
    false
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isDBAlive() -> bool {
    first_call("Aime_isDBAlive");
    ONLINE.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isMobile() -> bool {
    first_call("Aime_isMobile");
    false
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isReaderDetected() -> bool {
    first_call("Aime_isReaderDetected");
    ONLINE.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_sendLog(aime_id: u32, kind: u32, value: u32) -> bool {
    log!("aime: log {aime_id} {kind} {value}");
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_setLedError() -> bool {
    log!("aime: LED error");
    true
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_setLedSuccess() -> bool {
    log!("aime: LED success");
    true
}
