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

// The reader's workflow API (Road Fighters 3D): the game steers the reader through states
// (`ac_io_icca_workflow`) and polls its 24-byte status (`ac_io_icca_get_status`: state, solenoid,
// front and rear sensors, the card's id, error, keypad edges and levels). A press of the `card`
// control presents the card (`ACTIVE`, sensors on) until the game ejects it.

const WF_STEP: i32 = 0;
const WF_SLEEP: i32 = 1;
const WF_START: i32 = 2;
const WF_INIT: i32 = 3;
const WF_READY: i32 = 4;
const WF_GET_USERID: i32 = 5;
const WF_ACTIVE: i32 = 6;
const WF_EJECT: i32 = 7;
const WF_EJECT_CHECK: i32 = 8;
const WF_CLOSE_EJECT: i32 = 10;
const WF_CLOSE_END: i32 = 12;

static WORKFLOW: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(WF_STEP);
static CARD_HELD: AtomicBool = AtomicBool::new(false);

/// A press of the `card` control: the card is presented (or taken back while the reader waits
/// for its removal).
fn workflow_update() {
    let pressed = crate::hbhi::card_pressed();
    if pressed && !CARD_HELD.swap(true, Ordering::Relaxed) {
        let state = WORKFLOW.load(Ordering::Relaxed);
        if state == WF_EJECT_CHECK {
            INSERTED.store(false, Ordering::Relaxed);
            WORKFLOW.store(WF_SLEEP, Ordering::Relaxed);
            log!("icca: card taken");
        } else {
            INSERTED.store(true, Ordering::Relaxed);
            WORKFLOW.store(WF_ACTIVE, Ordering::Relaxed);
            log!("icca: card presented");
        }
    } else if !pressed {
        CARD_HELD.store(false, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_workflow(workflow: i32, _device: u32) -> i32 {
    let card = INSERTED.load(Ordering::Relaxed);
    let state = match workflow {
        WF_INIT => WF_READY,
        WF_START => if card { WF_ACTIVE } else { WF_READY },
        WF_EJECT => {
            INSERTED.store(false, Ordering::Relaxed);
            WORKFLOW.load(Ordering::Relaxed)
        }
        WF_CLOSE_EJECT => if card { WF_EJECT_CHECK } else { WF_SLEEP },
        WF_CLOSE_END => WF_SLEEP,
        WF_STEP | WF_SLEEP | WF_GET_USERID => workflow,
        _ => WORKFLOW.load(Ordering::Relaxed),
    };
    WORKFLOW.store(state, Ordering::Relaxed);
    state
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_status(out: *mut u8, _device: u32) -> bool {
    if out.is_null() {
        return false;
    }
    workflow_update();
    let mut status = [0u8; 24];
    status[0] = WORKFLOW.load(Ordering::Relaxed) as u8;
    if INSERTED.load(Ordering::Relaxed) {
        status[2] = 1;
        status[3] = 1;
        status[4..12].copy_from_slice(&card_id());
    }
    unsafe { std::ptr::copy_nonoverlapping(status.as_ptr(), out, status.len()) };
    true
}

/// The card's id and a FeliCa flag (byte 8: an e-Amusement card, `E004...`, is not one).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_uid_felica(_device: u32, out: *mut u8) -> bool {
    if out.is_null() {
        return false;
    }
    let id = if INSERTED.load(Ordering::Relaxed) { card_id() } else { [0; 8] };
    unsafe {
        std::ptr::copy_nonoverlapping(id.as_ptr(), out, 8);
        *out.add(8) = (id[0] != 0xe0) as u8 & (id != [0; 8]) as u8;
    }
    true
}
