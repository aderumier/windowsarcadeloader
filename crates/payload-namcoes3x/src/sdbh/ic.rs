//! The Hero License: the player's IC card (a MIFARE card the cabinet's reader reads and
//! writes, the hero's whole save on it), read through neodcd's `NeoDcdIC*` functions. Their
//! imports in the game are the payload's (neodcd's IC module never starts: it would look for
//! the reader and show error 6-11).
//!
//! The card is a `NeoDcdICCardData` (3540 bytes: serial, status, type, title and card ids, the
//! game's data, 3456 bytes at most). A new license (the dispenser's blank card, bought in the
//! game) is unformatted (status -4) for this title (game id 1036): the game formats it and
//! writes the hero on it. It is kept in `WindowsLoader\sdbh\hero_license.bin` of the dump
//! (`WALSDBH1` and the card's 3540 bytes), saved at each write.
//!
//! The card goes on the reader and comes off with the card button; the reader is index 0 of
//! every call.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::{iat, log};

pub const DATA_MAX: usize = 3456;
const CARD_SIZE: usize = 3540;
const MAGIC: &[u8; 8] = b"WALSDBH1";
const TITLE_ID: u32 = 1036;

const STATUS_UNFORMATTED: i32 = -4;
const STATUS_OK: i32 = 1;
const INIT_COMPLETED: i32 = 2;
const RW_OK: i32 = 1;
const WRITE_OK: i32 = 1;
const WRITE_NO_CARD: i32 = -2;

/// `NeoDcdICCardData`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CardData {
    serial: [u8; 16],
    status: i32,
    kind: i32,
    rest_count: u32,
    time: u32,
    game_id: u32,
    card_id: u32,
    campaign_id: u32,
    region_id: u32,
    data_size: u32,
    data: [u8; DATA_MAX],
    product_id: [u8; 8],
    access_code: [u8; 24],
}

const _: () = assert!(std::mem::size_of::<CardData>() == CARD_SIZE);

/// `NeoDcdICCardRawData`.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawData {
    data_size: u32,
    data: [u8; DATA_MAX],
}

struct Reader {
    card: Box<CardData>,
    raw: Box<RawData>,
    /// The last write's result.
    written: i32,
}

static READER: Mutex<Option<Reader>> = Mutex::new(None);
/// The card on the reader.
static PRESENT: AtomicBool = AtomicBool::new(false);

fn save_path() -> std::path::PathBuf {
    super::save_dir().join("hero_license.bin")
}

/// A new license: unformatted, its serial made from the time.
fn blank() -> Box<CardData> {
    let mut card: Box<CardData> = unsafe { Box::new(std::mem::zeroed()) };
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
    card.serial[..2].copy_from_slice(b"WL");
    for b in &mut card.serial[2..] {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = (seed >> 56) as u8;
    }
    card.status = STATUS_UNFORMATTED;
    card.game_id = TITLE_ID;
    card.card_id = 1;
    card.product_id = *b"WALSDBH1";
    card
}

fn load() -> Box<CardData> {
    let path = save_path();
    match std::fs::read(&path) {
        Ok(bytes) if bytes.len() == MAGIC.len() + CARD_SIZE && &bytes[..8] == MAGIC => {
            let mut card: Box<CardData> = unsafe { Box::new(std::mem::zeroed()) };
            unsafe { std::ptr::copy_nonoverlapping(bytes[8..].as_ptr(), (&mut *card as *mut CardData).cast::<u8>(), CARD_SIZE) };
            log!("sdbh: Hero License loaded ({} bytes of game data)", card.data_size);
            card
        }
        Ok(_) => {
            log!("sdbh: {} is not a Hero License file: a new license (the file is left as is)", path.display());
            blank()
        }
        Err(_) => {
            log!("sdbh: no Hero License yet: a new one (bought in the game)");
            blank()
        }
    }
}

fn save(card: &CardData) {
    let path = save_path();
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(unsafe { std::slice::from_raw_parts((card as *const CardData).cast::<u8>(), CARD_SIZE) });
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // written beside then renamed: a crash never leaves half a license
    let tmp = path.with_extension("tmp");
    match std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &path)) {
        Ok(()) => log!("sdbh: Hero License saved ({} bytes of game data)", card.data_size),
        Err(e) => log!("sdbh: Hero License not saved: {}: {e}", path.display()),
    }
}

/// The card on the reader or off it.
pub fn toggle() {
    let on = !PRESENT.load(Ordering::Relaxed);
    PRESENT.store(on, Ordering::Relaxed);
    log!("sdbh: Hero License {} the reader", if on { "on" } else { "off" });
}

pub fn present() -> bool {
    PRESENT.load(Ordering::Relaxed)
}

pub fn init(exe: usize) {
    *READER.lock().unwrap() = Some(Reader {
        card: load(),
        raw: unsafe { Box::new(std::mem::zeroed()) },
        written: WRITE_OK,
    });
    let hooks: [(&str, usize); 27] = [
        ("NeoDcdICInit", ic_init as *const () as usize),
        ("NeoDcdICGetInitStatus", ic_init_status as *const () as usize),
        ("NeoDcdICGetStatus", ic_ok as *const () as usize),
        ("NeoDcdICGetMode", ic_ok as *const () as usize),
        ("NeoDcdICConfigSupportedCard", ic_ignore1 as *const () as usize),
        ("NeoDcdICGetRWStatus", ic_rw_status as *const () as usize),
        ("NeoDcdICGetTestResult", ic_rw_status as *const () as usize),
        ("NeoDcdICReadWriteTest", ic_rw_status as *const () as usize),
        ("NeoDcdICGetAvailableCards", ic_available as *const () as usize),
        ("NeoDcdICGetCardByIndex", ic_card_by_index as *const () as usize),
        ("NeoDcdICGetCardBySerial", ic_card_by_serial as *const () as usize),
        ("NeoDcdICGetCardRawDataByIndex", ic_raw_by_index as *const () as usize),
        ("NeoDcdICGetCardRawDataBySerial", ic_raw_by_serial as *const () as usize),
        ("NeoDcdICGetAvailableTags", ic_none1 as *const () as usize),
        ("NeoDcdICGetTagByIndex", ic_none2 as *const () as usize),
        ("NeoDcdICGetTagBySerial", ic_none2 as *const () as usize),
        ("NeoDcdICGetTagRawDataByIndex", ic_none2 as *const () as usize),
        ("NeoDcdICGetTagRawDataBySerial", ic_none2 as *const () as usize),
        ("NeoDcdICWriteCard", ic_write as *const () as usize),
        ("NeoDcdICWriteCardWithoutBackup", ic_write as *const () as usize),
        ("NeoDcdICReformatCard", ic_reformat as *const () as usize),
        ("NeoDcdICTransferCard", ic_transfer as *const () as usize),
        ("NeoDcdICWriteTag", ic_write_tag as *const () as usize),
        ("NeoDcdICGetWriteResult", ic_write_result as *const () as usize),
        ("NeoDcdICGetBackupedCardCount", ic_none0 as *const () as usize),
        ("NeoDcdICGetBackupedCard", ic_none1 as *const () as usize),
        ("NeoDcdICClearBackupedCard", ic_none0 as *const () as usize),
    ];
    for (name, f) in hooks {
        if unsafe { iat::hook_module(exe, "neodcd.dll", name, f) }.is_none() {
            log!("sdbh: the game does not import {name}");
        }
    }
}

unsafe extern "system" fn ic_init(_a: usize, _b: usize) -> i32 {
    log!("sdbh: IC card reader: the Hero License");
    1
}
unsafe extern "system" fn ic_init_status() -> i32 {
    INIT_COMPLETED
}
unsafe extern "system" fn ic_ok() -> i32 {
    STATUS_OK
}
unsafe extern "system" fn ic_ignore1(_a: usize) {}
unsafe extern "system" fn ic_rw_status(_rw: i32) -> i32 {
    RW_OK
}
unsafe extern "system" fn ic_none0() -> usize {
    0
}
unsafe extern "system" fn ic_none1(_a: usize) -> usize {
    0
}
unsafe extern "system" fn ic_none2(_a: usize, _b: usize) -> usize {
    0
}

unsafe extern "system" fn ic_available(_rw: i32) -> i32 {
    present() as i32
}

/// The card's serial is `serial` (16 bytes).
fn matches(card: &CardData, serial: *const u8) -> bool {
    !serial.is_null() && unsafe { std::slice::from_raw_parts(serial, 16) } == card.serial
}

unsafe extern "system" fn ic_card_by_index(_rw: i32, index: i32) -> *const CardData {
    let r = READER.lock().unwrap();
    match r.as_ref() {
        Some(r) if present() && index == 0 => &*r.card,
        _ => std::ptr::null(),
    }
}

unsafe extern "system" fn ic_card_by_serial(_rw: i32, serial: *const u8) -> *const CardData {
    let r = READER.lock().unwrap();
    match r.as_ref() {
        Some(r) if present() && matches(&r.card, serial) => &*r.card,
        _ => std::ptr::null(),
    }
}

fn raw(r: &mut Reader) -> *const RawData {
    r.raw.data_size = r.card.data_size;
    r.raw.data = r.card.data;
    &*r.raw
}

unsafe extern "system" fn ic_raw_by_index(_rw: i32, index: i32) -> *const RawData {
    let mut r = READER.lock().unwrap();
    match r.as_mut() {
        Some(r) if present() && index == 0 => raw(r),
        _ => std::ptr::null(),
    }
}

unsafe extern "system" fn ic_raw_by_serial(_rw: i32, serial: *const u8) -> *const RawData {
    let mut r = READER.lock().unwrap();
    match r.as_mut() {
        Some(r) if present() && matches(&r.card, serial) => raw(r),
        _ => std::ptr::null(),
    }
}

/// The game's data written on the card from `offset` (`format`: the card formatted first).
fn write(serial: *const u8, data: *const u8, size: u32, offset: u32, format: bool) -> i32 {
    let mut guard = READER.lock().unwrap();
    let Some(r) = guard.as_mut() else { return 0 };
    let (size, offset) = (size as usize, offset as usize);
    log!("sdbh: Hero License {} {size} bytes at {offset}", if format { "formatted with" } else { "written" });
    if !present() || !matches(&r.card, serial) || data.is_null() || offset + size > DATA_MAX {
        r.written = WRITE_NO_CARD;
        return 0;
    }
    if format {
        r.card.data = [0; DATA_MAX];
        r.card.data_size = 0;
    }
    r.card.data[offset..offset + size].copy_from_slice(unsafe { std::slice::from_raw_parts(data, size) });
    r.card.data_size = r.card.data_size.max((offset + size) as u32);
    r.card.status = STATUS_OK;
    r.written = WRITE_OK;
    save(&r.card);
    1
}

unsafe extern "system" fn ic_write(_rw: i32, serial: *const u8, data: *const u8, size: u32, offset: u32) -> i32 {
    write(serial, data, size, offset, false)
}

unsafe extern "system" fn ic_reformat(_rw: i32, serial: *const u8, data: *const u8, size: u32, offset: u32) -> i32 {
    write(serial, data, size, offset, true)
}

/// A license's hero moved to a new card: one card here, nothing to move.
unsafe extern "system" fn ic_transfer(_rw: i32, _from: *const u8, _to: *const u8, _data: *const u8, _size: u32, _keep: i32) -> i32 {
    log!("sdbh: Hero License transfer asked for: one license here");
    0
}

unsafe extern "system" fn ic_write_tag(_rw: i32, _serial: *const u8, _data: *const u8, _size: u32, _offset: u32) -> i32 {
    0
}

unsafe extern "system" fn ic_write_result(_rw: i32, _a: usize, _b: usize) -> i32 {
    READER.lock().unwrap().as_ref().map_or(WRITE_OK, |r| r.written)
}
