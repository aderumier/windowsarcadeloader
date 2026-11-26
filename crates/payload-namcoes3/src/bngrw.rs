//! Banapass card reader of the `bngrw.dll` API (Wangan Midnight Maximum Tune 6, 64-bit): the
//! game's imports of the library hooked (the dump's own library drives the real reader).
//!
//! The game polls for a card tap ([`wait_touch`]): with the player's card on the reader
//! ([`insert`]) each request is answered with the card's record ([`TEMPLATE`]): chip id (+0x2c,
//! 32 characters) and access code (+0x50, 20 digits).
//!
//! Mobile Suit Gundam Extreme Vs. 2 ([`init_callbacks`]) makes one tap request and waits for its
//! callback: the card's record goes to the request's callback every 100 ms while the card is on
//! the reader, until the next request (EXVS2-POC's `banapass.cpp`); its action requests (the
//! reader's LED / sound sequences) are answered through their callback too.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{iat, log};

type ReplyCallback = unsafe extern "C" fn(i32, i32, *mut c_void) -> isize;
type TouchCallback = unsafe extern "C" fn(i32, i32, *mut c_void, *mut c_void);

/// The card on the reader, until when.
static CARD: Mutex<Option<(&'static crate::banapass::Card, Instant)>> = Mutex::new(None);
/// Puts `card` on the reader for `time`: every tap request meanwhile gets it (the game clears
/// its copy of the card at each request, until it has used it).
pub(crate) fn insert(card: &'static crate::banapass::Card, time: Duration) {
    *CARD.lock().unwrap() = Some((card, Instant::now() + time));
}

/// The reader's card record (openBanaW5p's, https://github.com/kritbualad/OpenBanapassW5P):
/// a Mifare card of the reader "NBGIC6", its chip id and access code in place of the zeros.
const TEMPLATE: [u8; 168] = [
    0x01, 0x01, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x92, 0x2E, 0x58, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x7F, 0x5C, 0x97, 0x44, 0xF0, 0x88, 0x04, 0x00, //
    0x43, 0x26, 0x2C, 0x33, 0x00, 0x04, 0x06, 0x10, 0x30, 0x30, 0x30, 0x30, //
    0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, //
    0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, //
    0x30, 0x30, 0x30, 0x30, 0x00, 0x00, 0x00, 0x00, 0x30, 0x30, 0x30, 0x30, //
    0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, //
    0x30, 0x30, 0x30, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, //
    0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x4E, 0x42, 0x47, 0x49, 0x43, 0x36, 0x00, 0x00, 0xFA, 0xE9, 0x69, 0x00, //
    0xF6, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
];

/// The card record of the callback.
fn record(card: &crate::banapass::Card) -> [u8; 168] {
    let mut data = TEMPLATE;
    let mut chip = String::new();
    for b in card.uid().iter().chain(card.block(1)[..12].iter()) {
        chip += &format!("{b:02X}");
    }
    data[0x2c..0x4c].copy_from_slice(&chip.as_bytes()[..32]);
    data[0x4c] = 0;
    let code = card.access_code.as_bytes();
    let n = code.len().min(20);
    data[0x50..0x50 + n].copy_from_slice(&code[..n]);
    data[0x50 + n] = 0;
    data
}

/// The game's reader object (the request's context): its result status (+0x30, 0: a card) and
/// its copy of the card record (+0x38), what the game's callback stores.
const CONTEXT_STATUS: usize = 0x30;
const CONTEXT_RECORD: usize = 0x38;

/// [`init_callbacks`]: the card goes to the requests' callbacks.
static CALLBACK_MODE: AtomicBool = AtomicBool::new(false);
/// The current tap request (callback mode): an older request's thread stops.
static REQUEST: AtomicU32 = AtomicU32::new(0);

/// A tap request in callback mode: the card's record to `callback` every 100 ms while the card is
/// on the reader, until the next request.
fn touch_thread(callback: TouchCallback, context: usize) {
    let id = REQUEST.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        while REQUEST.load(Ordering::SeqCst) == id {
            std::thread::sleep(Duration::from_millis(100));
            let card = CARD.lock().unwrap().filter(|(_, until)| Instant::now() < *until).map(|(c, _)| c);
            let Some(card) = card else { continue };
            if REQUEST.load(Ordering::SeqCst) != id {
                break;
            }
            static LOGGED: Mutex<Option<Instant>> = Mutex::new(None);
            {
                let mut logged = LOGGED.lock().unwrap();
                if logged.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
                    log!("bngrw: card {} on the reader", card.access_code);
                    *logged = Some(Instant::now());
                }
            }
            let mut record = record(card);
            unsafe { callback(0, 0, record.as_mut_ptr().cast(), context as *mut c_void) };
        }
    });
}

unsafe extern "system" fn wait_touch(_reader: u32, _timeout: i32, _flags: u32, callback: Option<TouchCallback>, context: *mut c_void) -> i32 {
    if CALLBACK_MODE.load(Ordering::Relaxed) {
        if std::env::var("WAL_NAMCOES3_CARD_TRACE").is_ok_and(|v| v == "1") {
            log!("bngrw: tap request (callback {})", callback.is_some());
        }
        if let Some(callback) = callback {
            touch_thread(callback, context as usize);
        }
        return 1;
    }
    let card = CARD.lock().unwrap().filter(|(_, until)| Instant::now() < *until).map(|(c, _)| c);
    let Some(card) = card else { return -1 };
    if context.is_null() {
        return -1;
    }
    static LOGGED: Mutex<Option<Instant>> = Mutex::new(None);
    let mut logged = LOGGED.lock().unwrap();
    if logged.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
        log!("bngrw: card {} on the reader", card.access_code);
        *logged = Some(Instant::now());
    }
    drop(logged);
    // the game reads the card as soon as the request returns, while it holds the lock its
    // callback takes: the callback's stores done here, under its lock
    let record = record(card);
    unsafe {
        std::ptr::copy_nonoverlapping(record.as_ptr(), context.cast::<u8>().add(CONTEXT_RECORD), record.len());
        *context.cast::<u8>().add(CONTEXT_STATUS).cast::<i32>() = 0;
    }
    1
}

/// The asynchronous requests the game makes before its taps (attach, reset): answered OK after
/// a moment.
fn reply_later(callback: Option<ReplyCallback>, context: *mut c_void) {
    if let Some(callback) = callback {
        let context = context as usize;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            unsafe { callback(0, 0, context as *mut c_void) };
        });
    }
}

unsafe extern "system" fn attach(_reader: u32, _name: *const u8, _a: i32, _b: i32, callback: Option<ReplyCallback>, context: *mut c_void) -> u64 {
    reply_later(callback, context);
    1
}

unsafe extern "system" fn cancel(_reader: u32) -> i32 {
    1
}

unsafe extern "system" fn zero() -> i32 {
    0
}

unsafe extern "system" fn version() -> u32 {
    0x0001_0000
}

type ActionCallback = unsafe extern "C" fn(i32, i32, *mut c_void);

/// `BngRwReqAction(reader, action, callback, context)` in callback mode: done, its callback
/// called after a moment (1, or -1 for an action other than 0, as EXVS2-POC answers).
unsafe extern "system" fn action(_reader: u32, which: u32, callback: Option<ActionCallback>, context: *mut c_void) -> i32 {
    if let Some(callback) = callback {
        let context = context as usize;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            unsafe { callback(0, 0, context as *mut c_void) };
        });
    }
    if which == 0 { 1 } else { -1 }
}

/// The reader's LED, beeper, URL and action requests: done.
unsafe extern "system" fn done(_a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    1
}

unsafe extern "system" fn idle(_cmd: u32) -> i32 {
    0
}

/// [`init`], the card and the action requests answered through their callbacks (Mobile Suit
/// Gundam Extreme Vs. 2).
pub(crate) fn init_callbacks(base: usize) {
    CALLBACK_MODE.store(true, Ordering::Relaxed);
    init(base);
}

/// Hooks the game's `bngrw.dll` imports.
pub(crate) fn init(base: usize) {
    let action_hook = if CALLBACK_MODE.load(Ordering::Relaxed) { action as *const () as usize } else { done as *const () as usize };
    let hooks: [(&str, usize); 11] = [
        ("BngRwInit", zero as *const () as usize),
        ("BngRwFin", zero as *const () as usize),
        ("BngRwGetVersion", version as *const () as usize),
        ("BngRwAttach", attach as *const () as usize),
        ("BngRwReqWaitTouch", wait_touch as *const () as usize),
        ("BngRwReqCancel", cancel as *const () as usize),
        ("BngRwIsCmdExec", idle as *const () as usize),
        ("BngRwReqBeep", done as *const () as usize),
        ("BngRwReqLed", done as *const () as usize),
        ("BngRwReqSendUrlTo", done as *const () as usize),
        ("BngRwReqAction", action_hook),
    ];
    for (name, f) in hooks {
        if unsafe { iat::hook_module(base, "bngrw.dll", name, f) }.is_none() {
            log!("bngrw: {name} not hooked");
        }
    }
    log!("bngrw: card reader");
}
