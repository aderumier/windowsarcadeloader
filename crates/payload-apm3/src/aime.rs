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
/// The last card read stays readable (`Aime_getAimeId`, `Aime_getAccessCode`) until the next
/// `Aime_start`, after `Aime_cancel` too (Otoshu DX cancels the reader, then asks the id).
static READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A card's result is readable.
fn has_card() -> bool {
    poll() == Reader::Done || READ.load(Ordering::Relaxed)
}

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
            READ.store(true, Ordering::Relaxed);
        }
    }
    *reader
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_start() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_start");
        if !ONLINE.load(Ordering::Relaxed) {
            return false;
        }
        log!("aime: waiting for a card");
        // a card already held is read too (the player pressed the card input before the prompt)
        *READER.lock().unwrap() = Reader::Waiting { released: true };
        READ.store(false, Ordering::Relaxed);
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_cancel() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_cancel");
        let mut reader = READER.lock().unwrap();
        if *reader != Reader::Idle {
            log!("aime: cancelled");
        }
        *reader = Reader::Idle;
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isBusy() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_isBusy");
        matches!(poll(), Reader::Waiting { .. })
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_hasResult() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_hasResult");
        poll() == Reader::Done
    })() as u32
}

/// `AccessCode`: 10 bytes (the 20 digits as BCD), then the digits as a NUL terminated UTF-16
/// string (21 characters).
#[unsafe(no_mangle)]
pub extern "system" fn Aime_getAccessCode(out: *mut u8) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_getAccessCode");
        if out.is_null() || !has_card() {
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
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_getAimeId(out: *mut u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_getAimeId");
        let read = has_card();
        if !out.is_null() {
            unsafe { *out = if read { aime_id() } else { u32::MAX } };
        }
        read
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_acceptConfirm() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_acceptConfirm");
        false
    })() as u32
}

#[unsafe(no_mangle)]
/// The card's database (`AimeConfirm`: 0 NoneDB, 1 FeliCaDB, 2 AimeDB): an Aime card registered in
/// the Aime database (Dengeki Bunko reads it after the card: NoneDB is "the card could not be
/// read").
pub extern "system" fn Aime_getConfirm() -> u32 {
    first_call("Aime_getConfirm");
    if has_card() { 2 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_hasConfirm() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_hasConfirm");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_getErrorCategory() -> u32 {
    first_call("Aime_getErrorCategory");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_hasError() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_hasError");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isDBAlive() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_isDBAlive");
        ONLINE.load(Ordering::Relaxed)
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isMobile() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_isMobile");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_isReaderDetected() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Aime_isReaderDetected");
        ONLINE.load(Ordering::Relaxed)
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_sendLog(aime_id: u32, kind: u32, value: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("aime: log {aime_id} {kind} {value}");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_setLedError() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("aime: LED error");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Aime_setLedSuccess() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("aime: LED success");
        true
    })() as u32
}
