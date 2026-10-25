//! The ICCA e-Amusement card reader (ACIO device 1), a slot reader: a press of the `card`
//! control inserts the card, the next one ejects it. Its 8-byte id is `WAL_KONAMI_CARD_ID`
//! (16 hex digits), by default an e-Amusement card id (`E004...`). A card configured in the GUI
//! (`WAL_KONAMI_CARD_PIN` set) starts inserted, unless `WAL_KONAMI_CARD_AUTO=0`. A second
//! reader (device 2) holds player 2's card (`WAL_KONAMI_CARD2_ID`).

use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::log;

/// `ac_io_icca_cardunit_init_isfinished`: the reader is ready.
const UNIT_READY: u32 = 4;
/// `ac_io_icca_req_uid_isfinished`: a card in the slot, its id readable.
const CARD_PRESENT: u32 = 9;
/// `ac_io_icca_req_uid_isfinished`: no card.
const NO_CARD: u32 = 0xb;

static INSERTED: AtomicBool = AtomicBool::new(false);
/// Metal Gear Arcade's status layout (`set_compat`).
static COMPAT: AtomicBool = AtomicBool::new(false);

/// Metal Gear Arcade's reader (spice2x's compat mode): a touch reader, its status the state
/// START (2) while a card is on it (else CLOSE_EJECT), the card's id at byte 2 (not 4), the
/// keypad's level at 12 (<< 16), edge at 16, level at 20. The game asks the card's PIN on the
/// reader's keypad: the player's numpad (`kp0`..`kp9`), or the configured PIN
/// (`WAL_KONAMI_CARD_PIN`) typed there by itself. A card
/// touch lasts 2 s (the game then waits for the card to leave): the card key, or with a
/// configured card (`auto_insert`) a press of START (the attract's START opens the player
/// entry, which reads it).
pub(crate) fn set_compat() {
    COMPAT.store(true, Ordering::Relaxed);
}

const TOUCH: std::time::Duration = std::time::Duration::from_secs(2);
static TOUCHED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
static TOUCH_KEYS_HELD: AtomicBool = AtomicBool::new(false);

/// The keypad's key codes and numbers (spice2x's), index = the digit (0-9), 10 the decimal
/// point, 11 the `00` key.
const KEY_CODES: [u32; 12] = [0x100, 0x200, 0x2000, 2, 0x400, 0x4000, 4, 0x800, 0x8000, 8, 1, 0x1000];
const KEY_NUMS: [u32; 12] = [0, 1, 5, 9, 2, 6, 10, 3, 7, 11, 8, 4];
/// The PIN typed from this long after the touch (the PIN screen shows about 10 s after it, once
/// the card's login is done), again every `PIN_REPEAT` until `PIN_END` (a slower login), each
/// digit pressed and released this long.
const PIN_DELAY: std::time::Duration = std::time::Duration::from_secs(10);
const PIN_REPEAT: u128 = 4000;
const PIN_END: u128 = 20000;
const PIN_KEY: std::time::Duration = std::time::Duration::from_millis(200);
/// Per player's reader (0: player 1, 1: player 2).
static KEY_EDGE: [std::sync::atomic::AtomicU32; 2] = [const { std::sync::atomic::AtomicU32::new(0) }; 2];
static KEY_SERIAL: [std::sync::atomic::AtomicU32; 2] = [const { std::sync::atomic::AtomicU32::new(0) }; 2];
static KEY_DOWN: [std::sync::atomic::AtomicI32; 2] = [const { std::sync::atomic::AtomicI32::new(-1) }; 2];

/// When the configured PIN starts being typed from (`PIN_DELAY` after the first touch of a
/// login: later touches, within `PIN_DELAY + PIN_END`, do not restart it).
static PIN_FROM: [std::sync::Mutex<Option<std::time::Instant>>; 2] = [const { std::sync::Mutex::new(None) }; 2];

/// Hello! Pop'n Music's readers (`set_pin_on_poll`): the game polls a reader's keypad
/// (`ac_io_icca_get_status`) only while its PIN screen is up: the configured PIN is typed once,
/// 1 s after the polling starts (again after a 3 s pause in it).
static PIN_ON_POLL: AtomicBool = AtomicBool::new(false);
static LAST_POLL: [std::sync::Mutex<Option<std::time::Instant>>; 2] = [const { std::sync::Mutex::new(None) }; 2];

pub(crate) fn set_pin_on_poll() {
    PIN_ON_POLL.store(true, Ordering::Relaxed);
}

/// A poll of `reader`'s keypad: the PIN typing armed when the polling starts.
fn keypad_polled(reader: usize) {
    if !PIN_ON_POLL.load(Ordering::Relaxed) {
        return;
    }
    let now = std::time::Instant::now();
    let last = LAST_POLL[reader].lock().unwrap().replace(now);
    if last.is_none_or(|t| now.duration_since(t).as_secs() >= 3) {
        // typed from PIN_DELAY on: 1 s from now
        *PIN_FROM[reader].lock().unwrap() = now.checked_sub(PIN_DELAY - std::time::Duration::from_secs(1));
        log!("icca: reader {} keypad polled, its PIN typed", reader + 1);
    }
}

/// The digit the keypad has down: the player's (the `kp0`..`kp9` controls: the keyboard's
/// numpad), else the configured PIN's (`WAL_KONAMI_CARD_PIN`) as typed after a touch.
fn keypad_key(reader: usize) -> Option<usize> {
    let stick = wal_payload_common::input(reader);
    if let Some(d) = (0..10).find(|&d| stick.pressed(wal_protocol::button::KP0 << d)) {
        return Some(d);
    }
    let pin = std::env::var(if reader == 0 { "WAL_KONAMI_CARD_PIN" } else { "WAL_KONAMI_CARD2_PIN" }).unwrap_or_default();
    let digits: Vec<usize> = pin.trim().bytes().filter(u8::is_ascii_digit).map(|b| (b - b'0') as usize).collect();
    // typed once on a polled keypad, again every PIN_REPEAT on a touch reader
    let end = if PIN_ON_POLL.load(Ordering::Relaxed) { PIN_REPEAT } else { PIN_END };
    PIN_FROM[reader].lock().unwrap().and_then(|t| {
        let since = t.elapsed().checked_sub(PIN_DELAY)?.as_millis();
        if since >= end {
            return None;
        }
        let slot = ((since % PIN_REPEAT) / PIN_KEY.as_millis()) as usize;
        // even slots: a digit down, odd ones: released
        (slot % 2 == 0).then(|| digits.get(slot / 2).copied()).flatten()
    })
}

/// The keypad's `(edge, level)` words (spice2x's encoding): the level the key down's code, the
/// edge (kept after the release, its code cleared) the last press: code << 16, 0x80, a 3-bit
/// serial << 4, the key's number.
fn keypad(reader: usize) -> (u32, u32) {
    let key = keypad_key(reader);
    let down = key.map(|k| k as i32).unwrap_or(-1);
    let was = KEY_DOWN[reader].swap(down, Ordering::Relaxed);
    match key {
        Some(k) => {
            if was != down {
                let serial = KEY_SERIAL[reader].fetch_add(1, Ordering::Relaxed) & 7;
                KEY_EDGE[reader].store((KEY_CODES[k] << 16) | 0x80 | (serial << 4) | KEY_NUMS[k], Ordering::Relaxed);
                log!("icca: keypad {k}");
            }
            (KEY_EDGE[reader].load(Ordering::Relaxed), KEY_CODES[k])
        }
        None => {
            let edge = KEY_EDGE[reader].load(Ordering::Relaxed) & 0xffff;
            KEY_EDGE[reader].store(edge, Ordering::Relaxed);
            (edge, 0)
        }
    }
}

/// A touch reader's card: on it for 2 s after the card key (or START, a configured card).
fn touched() -> bool {
    let stick = wal_payload_common::input(0);
    let key = stick.pressed(wal_protocol::button::CARD) || (auto_insert() && stick.pressed(wal_protocol::button::START));
    let mut touched = TOUCHED.lock().unwrap();
    if key && !TOUCH_KEYS_HELD.swap(true, Ordering::Relaxed) {
        let now = std::time::Instant::now();
        *touched = Some(now);
        let mut pin_from = PIN_FROM[0].lock().unwrap();
        if pin_from.is_none_or(|t| t.elapsed() > PIN_DELAY + std::time::Duration::from_millis(PIN_END as u64)) {
            *pin_from = Some(now);
        }
        log!("icca: card touched");
    } else if !key {
        TOUCH_KEYS_HELD.store(false, Ordering::Relaxed);
    }
    touched.is_some_and(|t| t.elapsed() < TOUCH)
}
static HELD: AtomicBool = AtomicBool::new(false);

pub(crate) fn card_id() -> [u8; 8] {
    let mut id = [0xe0, 0x04, 0x01, 0x00, 0x27, 0x10, 0x4b, 0x1d];
    if let Ok(text) = std::env::var("WAL_KONAMI_CARD_ID") {
        let text = text.trim();
        if text.is_empty() {
            return id;
        }
        if text.len() == 16 {
            for (i, b) in id.iter_mut().enumerate() {
                match u8::from_str_radix(&text[i * 2..i * 2 + 2], 16) {
                    Ok(v) => *b = v,
                    Err(_) => break,
                }
            }
        } else {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| log!("icca: WAL_KONAMI_CARD_ID must be 16 hex digits, default card used"));
        }
    }
    id
}

fn card_pressed() -> bool {
    wal_payload_common::input(0).pressed(wal_protocol::button::CARD)
}

/// Whether the card starts inserted: a card configured (its PIN set), `WAL_KONAMI_CARD_AUTO`
/// not `0`.
fn auto_insert() -> bool {
    let pin = std::env::var("WAL_KONAMI_CARD_PIN").is_ok_and(|p| !p.trim().is_empty());
    let auto = !std::env::var("WAL_KONAMI_CARD_AUTO").is_ok_and(|v| v.trim() == "0");
    pin && auto
}

/// A configured card in the reader from the first look at it.
fn auto_start() {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        if auto_insert() {
            INSERTED.store(true, Ordering::Relaxed);
            log!("icca: card inserted (configured card)");
        }
    });
}

/// The card slot after the latest `card` control state (a press toggles the card).
fn inserted() -> bool {
    auto_start();
    let pressed = card_pressed();
    if pressed && !HELD.swap(true, Ordering::Relaxed) {
        let now = !INSERTED.fetch_xor(true, Ordering::Relaxed);
        log!("icca: card {}", if now { "inserted" } else { "ejected" });
    } else if !pressed {
        HELD.store(false, Ordering::Relaxed);
    }
    INSERTED.load(Ordering::Relaxed)
}

// Player 2's reader (device 2: Hello! Pop'n Music has one per player, 1 and 2): player 2's card
// (`WAL_KONAMI_CARD2_ID`, none when unset), inserted / ejected with player 2's card control,
// in from the start with its PIN set (`WAL_KONAMI_CARD2_PIN`). Player 1's card is never there
// (in both readers, the game refuses it: "the same card cannot be used").

static INSERTED2: AtomicBool = AtomicBool::new(false);
static HELD2: AtomicBool = AtomicBool::new(false);

/// Player 2's card id, when configured.
fn card2_id() -> Option<[u8; 8]> {
    let text = std::env::var("WAL_KONAMI_CARD2_ID").ok()?;
    let text = text.trim();
    if text.len() != 16 {
        return None;
    }
    let mut id = [0u8; 8];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(id)
}

/// Player 2's reader's card: its id while inserted.
fn reader2_card() -> Option<[u8; 8]> {
    let id = card2_id()?;
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        let auto = !std::env::var("WAL_KONAMI_CARD_AUTO").is_ok_and(|v| v.trim() == "0");
        if auto && std::env::var("WAL_KONAMI_CARD2_PIN").is_ok_and(|p| !p.trim().is_empty()) {
            INSERTED2.store(true, Ordering::Relaxed);
            log!("icca: player 2's card inserted (configured card)");
        }
    });
    let pressed = wal_payload_common::input(1).pressed(wal_protocol::button::CARD);
    if pressed && !HELD2.swap(true, Ordering::Relaxed) {
        let now = !INSERTED2.fetch_xor(true, Ordering::Relaxed);
        log!("icca: player 2's card {}", if now { "inserted" } else { "ejected" });
    } else if !pressed {
        HELD2.store(false, Ordering::Relaxed);
    }
    INSERTED2.load(Ordering::Relaxed).then_some(id)
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
pub extern "C" fn ac_io_icca_req_uid_isfinished(device: u32, status: *mut u32) -> bool {
    if !status.is_null() {
        let card = if device == 2 { reader2_card().is_some() } else { inserted() };
        unsafe { *status = if card { CARD_PRESENT } else { NO_CARD } };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_uid(device: u32, out: *mut u8) -> bool {
    if out.is_null() {
        return false;
    }
    let id = if device == 2 { reader2_card().unwrap_or_default() } else { card_id() };
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
    let pressed = card_pressed();
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

/// The reader's status: `(out, device)`; some games pass `(device, out)` (spice2x's note on
/// Metal Gear Arcade): the pointer is the argument that is not a small device number.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_status(a: usize, b: usize) -> bool {
    let (out, device) = if b > 2 { (b, a) } else { (a, b) };
    let out = out as *mut u8;
    if out.is_null() || (out as usize) <= 2 {
        return false;
    }
    if COMPAT.load(Ordering::Relaxed) {
        let card = touched();
        let mut status = [0u8; 24];
        status[0] = if card { WF_START as u8 } else { WF_CLOSE_EJECT as u8 };
        if card {
            status[2..10].copy_from_slice(&card_id());
        }
        // the reader's keypad: the configured PIN typed after the touch
        let (edge, level) = keypad(0);
        status[12..16].copy_from_slice(&(level << 16).to_le_bytes());
        status[16..20].copy_from_slice(&edge.to_le_bytes());
        status[20..24].copy_from_slice(&level.to_le_bytes());
        unsafe { std::ptr::copy_nonoverlapping(status.as_ptr(), out, status.len()) };
        return true;
    }
    auto_start();
    workflow_update();
    let mut status = [0u8; 24];
    if device == 2 {
        let card = reader2_card();
        status[0] = if card.is_some() { WF_ACTIVE } else { WF_SLEEP } as u8;
        if let Some(id) = card {
            status[2] = 1;
            status[3] = 1;
            status[4..12].copy_from_slice(&id);
        }
        keypad_polled(1);
        let (edge, level) = keypad(1);
        status[16..20].copy_from_slice(&edge.to_le_bytes());
        status[20..24].copy_from_slice(&level.to_le_bytes());
        unsafe { std::ptr::copy_nonoverlapping(status.as_ptr(), out, status.len()) };
        return true;
    }
    status[0] = WORKFLOW.load(Ordering::Relaxed) as u8;
    if INSERTED.load(Ordering::Relaxed) {
        status[2] = 1;
        status[3] = 1;
        status[4..12].copy_from_slice(&card_id());
    }
    // the reader's keypad (the player's numpad)
    keypad_polled(0);
    let (edge, level) = keypad(0);
    status[16..20].copy_from_slice(&edge.to_le_bytes());
    status[20..24].copy_from_slice(&level.to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(status.as_ptr(), out, status.len()) };
    true
}

/// The card's id and a FeliCa flag (byte 8: an e-Amusement card, `E004...`, is not one).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_uid_felica(device: u32, out: *mut u8) -> bool {
    if out.is_null() {
        return false;
    }
    let id = match device {
        2 => reader2_card().unwrap_or_default(),
        _ if INSERTED.load(Ordering::Relaxed) => card_id(),
        _ => [0; 8],
    };
    unsafe {
        std::ptr::copy_nonoverlapping(id.as_ptr(), out, 8);
        *out.add(8) = (id[0] != 0xe0) as u8 & (id != [0; 8]) as u8;
    }
    true
}

// The later readers' setup (Bishi Bashi Channel): the card's encryption, its power, the
// keep-alive: done, no error.

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_crypt_init(_device: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_device_control_iccard_power_supply_on(_device: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_device_control_iccard_power_supply_off(_device: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_device_control_isfinished(_device: u32, _status: *mut u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_get_keep_alive_error(_device: u32, error: *mut u32) -> bool {
    if !error.is_null() {
        unsafe { *error = 0 };
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_send_keep_alive_packet(_a: u32, _b: u32, _c: u32) -> i32 {
    0
}

/// The card in player `reader`'s reader (0: player 1, 1: player 2), for the boards that carry
/// the readers themselves (Dance Dance Revolution's P3IO).
pub(crate) fn reader_card(reader: usize) -> Option<[u8; 8]> {
    if reader == 0 { inserted().then(card_id) } else { reader2_card() }
}

/// The digit down on player `reader`'s keypad (the numpad, or the configured PIN being typed).
pub(crate) fn keypad_digit(reader: usize) -> Option<usize> {
    keypad_key(reader)
}

/// Types player `reader`'s configured PIN from `delay` on, once (a reader whose PIN screen
/// follows its card's read: Dance Dance Revolution's).
pub(crate) fn type_pin(reader: usize, delay: std::time::Duration) {
    PIN_ON_POLL.store(true, Ordering::Relaxed);
    *PIN_FROM[reader].lock().unwrap() = (std::time::Instant::now() + delay).checked_sub(PIN_DELAY);
}
