//! The ICCA e-Amusement card reader (ACIO device 1), a slot reader: a press of the `card`
//! control inserts the card, the next one ejects it. Its 8-byte id is `WAL_KONAMI_CARD_ID`
//! (16 hex digits), by default an e-Amusement card id (`E004...`).

use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::log;

/// `ac_io_icca_cardunit_init_isfinished`: the reader is ready.
const UNIT_READY: u32 = 4;
/// `ac_io_icca_req_uid_isfinished`: a card in the slot, its id readable.
const CARD_PRESENT: u32 = 9;
/// `ac_io_icca_req_uid_isfinished`: no card.
const NO_CARD: u32 = 0xb;

static INSERTED: AtomicBool = AtomicBool::new(false);
static HELD: AtomicBool = AtomicBool::new(false);

fn card_id() -> [u8; 8] {
    let mut id = [0xe0, 0x04, 0x01, 0x00, 0x27, 0x10, 0x4b, 0x1d];
    if let Ok(text) = std::env::var("WAL_KONAMI_CARD_ID") {
        let text = text.trim();
        if text.len() == 16 {
            for (i, b) in id.iter_mut().enumerate() {
                match u8::from_str_radix(&text[i * 2..i * 2 + 2], 16) {
                    Ok(v) => *b = v,
                    Err(_) => break,
                }
            }
        } else {
            log!("icca: WAL_KONAMI_CARD_ID must be 16 hex digits, default card used");
        }
    }
    id
}

/// The card slot after the latest `card` control state (a press toggles the card).
fn inserted() -> bool {
    let pressed = crate::hbhi::card_pressed();
    if pressed && !HELD.swap(true, Ordering::Relaxed) {
        let now = !INSERTED.fetch_xor(true, Ordering::Relaxed);
        log!("icca: card {}", if now { "inserted" } else { "ejected" });
    } else if !pressed {
        HELD.store(false, Ordering::Relaxed);
    }
    INSERTED.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_cardunit_init(_device: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_cardunit_init_isfinished(_device: u32, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = UNIT_READY };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_req_uid(_device: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_req_uid_isfinished(_device: u32, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = if inserted() { CARD_PRESENT } else { NO_CARD } };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_uid(_device: u32, out: *mut u8) -> bool {
    if out.is_null() {
        return false;
    }
    let id = card_id();
    unsafe { std::ptr::copy_nonoverlapping(id.as_ptr(), out, 8) };
    true
}
