//! Half-Life 2: Survivor's NESYS card play without Taito's servers (`WAL_NESICA_CARD_ID`: the
//! launcher's NESiCA card, set when the GUI has one).
//!
//! The card: `nesys.dll`'s `NesysCard_GetState` / `GetID` answer "inserted" with the card number
//! of `g_Global` +0x4C6 when it is set (launcher.dll's `-cardid`, a development switch): the
//! card number is written there (a NESYS number: 4420 and the NESiCA number's last 12 digits),
//! with launcher.dll's `-nesys` flags (`m_bNesys` +0x21 off, +0x22 and the online flag +0x28 on),
//! once launcher.dll's start is over (it clears the number). No card reader. Its card thread
//! (`FUN_10002e20`) turns the request states into the game's (`HL2NET_GetState`: -1 new card,
//! 10 player loaded...), asks the player after a login and copies it to `g_Global`.
//!
//! The player service: `nesys.dll` asks the lobby server (HTTP CGIs) from a worker thread, the
//! game polls its state (`Nesys_GetState`: 1 login, 2 player, 3 register, 4 update, 5 card
//! update running, 6 done, 7 failed with `Nesys_GetError`; `Nesys_GetEndState` the last
//! request's kind). Its request functions are replaced (a jump at their entry) by answers at
//! once, the state globals (read from the getters' `mov eax,[addr]`) set as the worker would:
//! - `Nesys_LoginUser(card)`: done when the card has a player, else failed with -1 (not
//!   registered: the game registers a new player);
//! - `Nesys_RequestUser(card, NESYS_USER_DATA*)`: the saved player (the record and the game's
//!   user data in `g_Global`);
//! - `Nesys_RegistUser` / `Nesys_UpdateUser(NESYS_USER_DATA*)`: saved;
//! - `Nesys_UpdateCard` / `Nesys_VerupCard`: done; `Nesys_Attestation`: done.
//!
//! A player is `nesys\<card>.bin` in the game's data folder: the record (`NESYS_USER_DATA`,
//! 0x58 bytes: card 32, player id, name 32, race, job, sex, class, level), then `g_Global`'s
//! user data (+0xBC78..+0xC988: the record's fields, skin, emblem, missions, items, story and
//! battle records) and its stage flags (+0x5F8..+0x608), as the lobby server would send back.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{drive, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

const CARD_ID: usize = 0x4C6;
const CARD_ID_SIZE: usize = 0x20;
const NESYS_FLAG: usize = 0x21;
const NESYS_MODE: usize = 0x22;
const ONLINE: usize = 0x28;
/// `g_Global`'s regions saved with a player.
const REGIONS: [(usize, usize); 2] = [(0x5F8, 0x608), (0xBC78, 0xC988)];
const RECORD: usize = 0x58;

const RUNNING_LOGIN: i32 = 1;
const RUNNING_REQUEST: i32 = 2;
const RUNNING_REGIST: i32 = 3;
const RUNNING_UPDATE: i32 = 4;
const RUNNING_CARD: i32 = 5;
const DONE: i32 = 6;
const FAILED: i32 = 7;
const NOT_REGISTERED: i32 = -1;

/// `g_Global` and the state globals (state, error, end state).
static GLOBAL: AtomicUsize = AtomicUsize::new(0);
static STATE: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static CARD: Mutex<String> = Mutex::new(String::new());

/// The card's player service in `nesys.dll`, `g_Global` the game's settings (`ac.dll`).
pub fn init(g: *mut u8) {
    let Some(card) = std::env::var("WAL_NESICA_CARD_ID").ok().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) else {
        return;
    };
    let nesys = unsafe { GetModuleHandleA(c"nesys.dll".as_ptr().cast()) };
    if nesys.is_null() {
        log!("hl2nesys: nesys.dll not loaded");
        return;
    }
    let export = |name: &std::ffi::CStr| unsafe { GetProcAddress(nesys, name.as_ptr().cast()) }.map(|f| f as usize);
    // the state globals: each getter is `mov eax,[addr]; ret`
    for (i, name) in [c"?Nesys_GetState@@YAHXZ", c"?Nesys_GetError@@YAHXZ", c"?Nesys_GetEndState@@YAHXZ"].into_iter().enumerate() {
        let Some(f) = export(name) else { return };
        let code = unsafe { std::slice::from_raw_parts(f as *const u8, 6) };
        if code[0] != 0xA1 || code[5] != 0xC3 {
            log!("hl2nesys: unexpected {name:?} {code:02x?}");
            return;
        }
        STATE[i].store(u32::from_le_bytes([code[1], code[2], code[3], code[4]]) as usize, Ordering::Relaxed);
    }
    let replaced: [(&std::ffi::CStr, usize); 8] = [
        (c"?Nesys_Attestation@@YA_NXZ", attestation as *const () as usize),
        (c"?Nesys_LoginUser@@YAHPAD@Z", login_user as *const () as usize),
        (c"?Nesys_RequestUser@@YAHPADPAUNESYS_USER_DATA@@@Z", request_user as *const () as usize),
        (c"?Nesys_RegistUser@@YAHPAUNESYS_USER_DATA@@@Z", regist_user as *const () as usize),
        (c"?Nesys_UpdateUser@@YAHPAUNESYS_USER_DATA@@@Z", update_user as *const () as usize),
        (c"?Nesys_UpdateCard@@YAHPAD0@Z", update_card as *const () as usize),
        (c"?Nesys_VerupCard@@YAHPAD0@Z", update_card as *const () as usize),
        (c"?NesysCard_Eject@@YA_NXZ", eject as *const () as usize),
    ];
    for (name, f) in replaced {
        match export(name) {
            Some(at) => unsafe { jump(at, f) },
            None => log!("hl2nesys: no {name:?}"),
        }
    }
    GLOBAL.store(g as usize, Ordering::Relaxed);
    let card = nesys_card(&card);
    *CARD.lock().unwrap() = card.clone();
    insert_card();
    log!("hl2nesys: card {card}, players in {}", folder());
}

/// The card in the reader (launcher.dll's -cardid) and NESYS mode (its -nesys). Again once the
/// game runs ([`crate::hl2ac`]'s binds thread): launcher.dll clears the card number after
/// `NesysIncomeOpen`, before its own -cardid.
pub fn insert_card() {
    let g = GLOBAL.load(Ordering::Relaxed) as *mut u8;
    let card = CARD.lock().unwrap().clone();
    if g.is_null() || card.is_empty() {
        return;
    }
    let id: Vec<u8> = card.bytes().take(CARD_ID_SIZE - 1).collect();
    unsafe {
        std::ptr::write_bytes(g.add(CARD_ID), 0, CARD_ID_SIZE);
        std::ptr::copy_nonoverlapping(id.as_ptr(), g.add(CARD_ID), id.len());
        *g.add(NESYS_FLAG) = 0;
        *g.add(NESYS_MODE) = 1;
        *g.add(ONLINE) = 1;
    }
}

/// The NESYS card number of the launcher's card: `nesys.dll`'s card thread accepts only numbers
/// starting with 4410, 4411, 4420 or 4421 (else "invalid card", -5); a NESiCA number (7020...)
/// becomes 4420 and its last 12 digits (the same card, the same player).
fn nesys_card(card: &str) -> String {
    const PREFIXES: [&str; 4] = ["4410", "4411", "4420", "4421"];
    if PREFIXES.iter().any(|p| card.starts_with(p)) {
        return card.to_string();
    }
    let digits: String = card.chars().filter(|c| c.is_ascii_digit()).collect();
    let tail = &digits[digits.len().saturating_sub(12)..];
    format!("4420{tail:0>12}")
}

/// `at` jumps to `f` (the function replaced).
unsafe fn jump(at: usize, f: usize) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        (at as *mut u8).write(0xE9);
        ((at + 1) as *mut i32).write_unaligned(f.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
}

/// The request `kind` ended: `state` (done / failed) and `error`.
fn finish(kind: i32, state: i32, error: i32) {
    let set = |i: usize, v: i32| unsafe { (STATE[i].load(Ordering::Relaxed) as *mut i32).write_volatile(v) };
    set(2, kind);
    set(1, error);
    set(0, state);
}

fn folder() -> String {
    format!("{}\\nesys", drive::data_dir())
}

/// A card number's player file (its digits and letters only).
fn path(card: &str) -> String {
    let name: String = card.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{}\\{name}.bin", folder())
}

/// The card number of a request (its C string), else the inserted card's.
fn card_of(p: *const u8) -> String {
    if p.is_null() {
        return CARD.lock().unwrap().clone();
    }
    let s = unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned();
    if s.is_empty() { CARD.lock().unwrap().clone() } else { s }
}

/// Saves the player: its record (`data`) and `g_Global`'s user data.
fn save(card: &str, data: *const u8) {
    let g = GLOBAL.load(Ordering::Relaxed) as *const u8;
    let mut blob = vec![0u8; RECORD];
    if !data.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(data, blob.as_mut_ptr(), RECORD) };
    }
    for (from, to) in REGIONS {
        blob.extend_from_slice(unsafe { std::slice::from_raw_parts(g.add(from), to - from) });
    }
    let _ = std::fs::create_dir_all(folder());
    match std::fs::write(path(card), &blob) {
        Ok(()) => log!("hl2nesys: player of card {card} saved"),
        Err(e) => log!("hl2nesys: cannot save {}: {e}", path(card)),
    }
}

/// The saved player of `card`: its record, `g_Global`'s user data restored.
fn load(card: &str, data: *mut u8) -> bool {
    let Ok(blob) = std::fs::read(path(card)) else { return false };
    let size = RECORD + REGIONS.iter().map(|(a, b)| b - a).sum::<usize>();
    if blob.len() != size {
        log!("hl2nesys: {} is {} bytes, not {size}", path(card), blob.len());
        return false;
    }
    if !data.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(blob.as_ptr(), data, RECORD) };
    }
    let g = GLOBAL.load(Ordering::Relaxed) as *mut u8;
    let mut at = RECORD;
    for (from, to) in REGIONS {
        unsafe { std::ptr::copy_nonoverlapping(blob.as_ptr().add(at), g.add(from), to - from) };
        at += to - from;
    }
    true
}

unsafe extern "C" fn attestation() -> bool {
    log!("hl2nesys: attestation");
    true
}

unsafe extern "C" fn login_user(card: *const u8) -> i32 {
    let card = card_of(card);
    let known = std::path::Path::new(&path(&card)).exists();
    log!("hl2nesys: login of card {card}: {}", if known { "known player" } else { "new card" });
    if known { finish(RUNNING_LOGIN, DONE, 0) } else { finish(RUNNING_LOGIN, FAILED, NOT_REGISTERED) }
    0
}

unsafe extern "C" fn request_user(card: *const u8, data: *mut u8) -> i32 {
    let card = card_of(card);
    if load(&card, data) {
        log!("hl2nesys: player of card {card} loaded");
        finish(RUNNING_REQUEST, DONE, 0);
    } else {
        log!("hl2nesys: no player for card {card}");
        finish(RUNNING_REQUEST, FAILED, NOT_REGISTERED);
    }
    0
}

/// The record's card number (its first 32 bytes), else the inserted card's.
fn record_card(data: *const u8) -> String {
    if data.is_null() {
        return CARD.lock().unwrap().clone();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, 32) };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(32);
    let s = String::from_utf8_lossy(&bytes[..end]).into_owned();
    if s.is_empty() { CARD.lock().unwrap().clone() } else { s }
}

unsafe extern "C" fn regist_user(data: *const u8) -> i32 {
    let card = record_card(data);
    log!("hl2nesys: new player for card {card}");
    save(&card, data);
    finish(RUNNING_REGIST, DONE, 0);
    0
}

unsafe extern "C" fn update_user(data: *const u8) -> i32 {
    save(&record_card(data), data);
    finish(RUNNING_UPDATE, DONE, 0);
    0
}

unsafe extern "C" fn update_card(_card: *const u8, _version: *const u8) -> i32 {
    log!("hl2nesys: card update");
    finish(RUNNING_CARD, DONE, 0);
    0
}

unsafe extern "C" fn eject() -> bool {
    true
}
