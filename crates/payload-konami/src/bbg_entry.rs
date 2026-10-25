//! Rainbow Bomber Girl's e-Amusement card (BM-System's Entry module, `bbg_system.rs`): the card
//! in player 1's reader logs in, its player kept by the payload (`dev/nvram/bbg/<card>.bin` in
//! the game's run directory, `C:\wal\konami\rainbow-bombergirl`, kept across prefix updates:
//! the PIN and the game's own player record, `READ_PLAYER_DATA`).
//!
//! The game reads the card entry from BM-System's state only when BM-System launched it; run
//! alone it takes its debug path, which this module drives: its entry scene's manager
//! (`GameEntryStateManager`) is given the card's id (`cardID`, 16 hex digits) and its kind
//! (`debugCardType`: NEWCARD, a card without player, or CONTINUE), and its PIN screen
//! (`GameEntrtyState_PassAuthentication`) the card's PIN (`debugPass`, else 0123). Done from the
//! Entry exports, which the game calls on Unity's main thread every frame of its entry scene.
//!
//! - A new card: terms, a PIN typed twice (`EntrySetPassWordRegister_Request`), a name
//!   (`EntryPlayerDataRegister_Request`): the player is created, an empty record (the game's
//!   defaults: the starting girls and skills).
//! - A known card: its PIN, then the player's record (`EntryGetPlayerData`).
//! - The game's end: the game saves through the NetworkV2 module (its stand-in kept: the lobby,
//!   messages); the payload rebuilds the same save from the game's data
//!   (`ProcessPacketBridge.CreatePlayerSaveCard`, marshalled by `Marshal.StructureToPtr`) once
//!   it is sent (`EntryPlayerSaveEndNotice`) and applies it to the record: the player's class
//!   experience, title, last girl, play count, win streak, headphone volume, the girls opened,
//!   each girl's experience (and rank, `GirlRankData.GetRank`), chat set, skill deck and dress.
//!   Not kept yet: skills and dresses won, missions, events, titles won, chats, play records.
//!
//! The card is the e-Amusement card (`icca.rs`: `WAL_KONAMI_CARD_ID`, F4 inserts / ejects it, a
//! configured card in from the start); ejected: guest play. Its PIN, when configured
//! (`WAL_KONAMI_CARD_PIN`), is typed on the PIN screens by itself (their digits move: the
//! screen's key handler is called); else the player touches it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use wal_payload_common::log;

use crate::mono::{ARRAY_DATA, BOXED, Mono, P, mono, read, write};

/// The game's card entry states (`GameEntryStateManager.E_CARD_ENTRY_STATE`: BM-System's, with
/// DATA_VER_ERR before END).
const NONE: i32 = 0;
const NEW_PASS_WAIT: i32 = 2;
const PASS_WAIT: i32 = 3;
const IN_NEW_PLAYER_DATA: i32 = 4;
const END: i32 = 8;
/// E_CARD_ENTRY_REQUEST_STATE
const SUCCESS: i32 = 2;
const ERROR_PASS_NG: i32 = 4;
/// GameEntryStateManager.DEBUGCARDTYPE
const NEWCARD: i32 = 2;
const CONTINUE: i32 = 4;

/// `READ_PLAYER_DATA` (Mono's marshalled layout, 64-bit).
const RECORD_SIZE: usize = 19232;
const NAME: usize = 0;
const NAME_SIZE: usize = 40;
const PLAYER_ID: usize = 40;
const CLASS_EXP: usize = 44;
const GIRLS_OPEN: usize = 48;
const GIRLS_SEEN: usize = 56;
const PLAY_COUNT: usize = 64;
const WIN_STREAK: usize = 72;
const GIRLS: usize = 80;
const GIRL_SIZE: usize = 144;
const GIRL_COUNT: usize = 24;
const LAST_GIRL: usize = 3536;
const HEADPHONE: usize = 6204;
const TITLE: usize = 18360;
/// A girl's record: id (i16), rank (i16), experience, chat set (id, 10 buttons), skill deck
/// (id, 4 skills), dress (8 ids).
const GIRL_RANK: usize = 2;
const GIRL_EXP: usize = 4;
const GIRL_CHAT_SET: usize = 48;
const GIRL_DECK: usize = 92;
const GIRL_DRESS: usize = 112;

/// `SavePlayerData`.
const SAVE_SIZE: usize = 29112;
const SAVE_CLASS_EXP: usize = 8;
const SAVE_TITLE: usize = 12;
const SAVE_GIRL: usize = 16;
const SAVE_GIRLS_OPEN: usize = 24;
const SAVE_GIRLS_SEEN: usize = 32;
const SAVE_GIRLS: usize = 2056;
const SAVE_GIRL_SIZE: usize = 136;
/// A saved girl: id, experience won, chat set (id, 10 buttons), skill deck (id, 4 skills of
/// 12 bytes: active, index, id), dress (8 ids).
const SAVE_GIRL_EXP: usize = 4;
const SAVE_GIRL_CHAT_SET: usize = 8;
const SAVE_GIRL_DECK: usize = 52;
const SAVE_GIRL_DRESS: usize = 104;
const SAVE_HEADPHONE: usize = 7804;
const SAVE_WIN_STREAK: usize = 25768;

/// The player behind the card being entered.
struct Session {
    /// The card's id (16 hex digits).
    card: String,
    pin: Option<[u8; 4]>,
    record: Option<Vec<u8>>,
    /// The PIN screen given the card's PIN.
    pin_set: bool,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static STATE: AtomicI32 = AtomicI32::new(NONE);
/// The entry scene's manager the card was given to (one login per entry scene).
static ENTERED: AtomicUsize = AtomicUsize::new(0);

fn path(card: &str) -> std::path::PathBuf {
    std::path::Path::new("dev").join("nvram").join("bbg").join(format!("{card}.bin"))
}

const MAGIC: &[u8; 8] = b"WALBBG1\0";

/// The card's PIN and record, when it has a player.
fn load(card: &str) -> Option<([u8; 4], Vec<u8>)> {
    let data = std::fs::read(path(card)).ok()?;
    if data.len() != 12 + RECORD_SIZE || &data[..8] != MAGIC {
        log!("bbg: {} unreadable, a new card", path(card).display());
        return None;
    }
    Some((data[8..12].try_into().unwrap(), data[12..].to_vec()))
}

fn store(card: &str, pin: [u8; 4], record: &[u8]) {
    let file = path(card);
    let mut data = MAGIC.to_vec();
    data.extend_from_slice(&pin);
    data.extend_from_slice(record);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&file, data) {
        Ok(()) => log!("bbg: player saved ({})", file.display()),
        Err(e) => log!("bbg: {} not written: {e}", file.display()),
    }
}

fn hex(id: &[u8; 8]) -> String {
    id.iter().map(|b| format!("{b:02X}")).collect()
}

/// The game's classes and fields used here.
struct Game {
    manager: P,
    instance: P,
    card_id: P,
    card_type: P,
    current_state: P,
    pass_type: P,
    debug_pass: P,
    pass_input: P,
    pass_digit: P,
    regist_type: P,
    regist_input: P,
    regist_confirm: P,
    regist_message: P,
    regist_digit: P,
    find_object: P,
    create_save: P,
    to_ptr: P,
    get_rank: P,
}

unsafe impl Send for Game {}
unsafe impl Sync for Game {}

fn game() -> Option<&'static Game> {
    static GAME: std::sync::OnceLock<Option<Game>> = std::sync::OnceLock::new();
    GAME.get_or_init(|| {
        let m = mono()?;
        let image = m.image("Assembly-CSharp");
        let manager = m.class(image, "", "GameEntryStateManager");
        let pass = m.class(image, "", "GameEntrtyState_PassAuthentication");
        let regist = m.class(image, "", "GameEntrtyState_PassRegist");
        let object = m.class(m.image("UnityEngine.CoreModule"), "UnityEngine", "Object");
        let marshal = m.class(m.image("mscorlib"), "System.Runtime.InteropServices", "Marshal");
        let g = Game {
            manager,
            instance: m.field(manager, "instance"),
            card_id: m.field(manager, "cardID"),
            card_type: m.field(manager, "debugCardType"),
            current_state: m.field(manager, "mCurrentState"),
            pass_type: m.pin(m.type_object(pass)),
            debug_pass: m.field(pass, "debugPass"),
            pass_input: m.field(pass, "mInputNumList"),
            pass_digit: m.method(pass, "OnNum", 1),
            regist_type: m.pin(m.type_object(regist)),
            regist_input: m.field(regist, "mInputNumList"),
            regist_confirm: m.field(regist, "mConfirmInputNumList"),
            regist_message: m.field(regist, "mCurrentMessage"),
            regist_digit: m.method(regist, "OnNum", 1),
            find_object: m.method(object, "FindObjectOfType", 1),
            create_save: m.method(m.class(image, "", "ProcessPacketBridge"), "CreatePlayerSaveCard", 0),
            to_ptr: m.method(marshal, "StructureToPtr", 3),
            get_rank: m.method(m.class(image, "", "GirlRankData"), "GetRank", 1),
        };
        let all = [
            g.instance, g.card_id, g.card_type, g.current_state, g.pass_type, g.debug_pass, g.pass_input, g.pass_digit, g.regist_type,
            g.regist_input, g.regist_confirm, g.regist_message, g.regist_digit, g.find_object, g.create_save, g.to_ptr, g.get_rank,
        ];
        if all.iter().any(|p| p.is_null()) {
            log!("bbg: the game's card entry not found, guest play only");
            return None;
        }
        Some(g)
    })
    .as_ref()
}

/// The entry scene's manager (null outside it).
fn manager(m: &Mono, g: &Game) -> *mut u8 {
    let at = m.static_addr(g.manager, g.instance);
    if at.is_null() { at } else { unsafe { read::<usize>(at, 0) as *mut u8 } }
}

/// Once a frame of the entry scene (Unity's main thread): a card in the reader is given to the
/// scene's manager, the PIN screen its PIN.
fn update() {
    let (Some(m), Some(g)) = (mono(), game()) else { return };
    let manager = manager(m, g);
    if manager.is_null() {
        return;
    }
    if ENTERED.load(Ordering::Relaxed) != manager as usize {
        let Some(id) = crate::icca::reader_card(0) else { return };
        let card = hex(&id);
        ENTERED.store(manager as usize, Ordering::Relaxed);
        let known = load(&card);
        let kind = if known.is_some() { CONTINUE } else { NEWCARD };
        log!("bbg: card {card}: {}", if known.is_some() { "its player" } else { "a new card" });
        STATE.store(if known.is_some() { PASS_WAIT } else { NEW_PASS_WAIT }, Ordering::Relaxed);
        *SESSION.lock().unwrap() = Some(Session {
            pin: known.as_ref().map(|k| k.0),
            record: known.map(|k| k.1),
            card: card.clone(),
            pin_set: false,
        });
        m.set(manager as P, g.card_id, m.string(&card));
        let mut kind = kind;
        m.set(manager as P, g.card_type, &mut kind as *mut i32 as P);
    }
    type_pin(m, g, manager);
    // the PIN screen, once it is up: the card's PIN
    let mut session = SESSION.lock().unwrap();
    let Some(s) = session.as_mut() else { return };
    let Some(pin) = s.pin.filter(|_| !s.pin_set) else { return };
    let mut args = [g.pass_type];
    let Some(screen) = m.invoke(g.find_object, std::ptr::null_mut(), &mut args).filter(|o| !o.is_null()) else { return };
    let array = unsafe { read::<usize>(screen as *const u8, m.offset(g.debug_pass)) } as *mut u8;
    if array.is_null() {
        return;
    }
    for (i, digit) in pin.iter().enumerate() {
        unsafe { write::<i32>(array, ARRAY_DATA + i * 4, (*digit - b'0') as i32) };
    }
    s.pin_set = true;
}

/// GameEntryStateManager.GAMEENTRYSTATE
const PASS_AUTHENTICATION: i32 = 6;
const PASS_REGIST: i32 = 7;
/// GameEntrtyState_PassRegist.MESSAGEID
const INPUT: i32 = 0;
const INPUT_CONFIRM: i32 = 1;
/// A digit typed this often on the PIN screens' keypad.
const DIGIT_EVERY: std::time::Duration = std::time::Duration::from_millis(350);

/// The configured card's PIN (`WAL_KONAMI_CARD_PIN`, 4 digits).
fn configured_pin() -> Option<[u8; 4]> {
    let pin = std::env::var("WAL_KONAMI_CARD_PIN").ok()?;
    let pin = pin.trim().as_bytes();
    (pin.len() == 4 && pin.iter().all(u8::is_ascii_digit)).then(|| pin.try_into().unwrap())
}

/// The number of digits in a screen's `List<int>` field.
fn typed(m: &Mono, screen: P, field: P) -> Option<usize> {
    let list = unsafe { read::<usize>(screen as *const u8, m.offset(field)) } as *const u8;
    if list.is_null() {
        return None;
    }
    let size = m.field(m.class_of(list as P), "_size");
    (!size.is_null()).then(|| unsafe { read::<i32>(list, m.offset(size)) }.max(0) as usize)
}

/// The configured card's PIN typed on the PIN screens' keypad (its digits move at each
/// screen): the screen's own key handler (`OnNum`) called with each digit in turn, the new
/// card's PIN twice (input, confirmation).
fn type_pin(m: &Mono, g: &Game, manager: *mut u8) {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let Some(pin) = configured_pin() else { return };
    let state = unsafe { read::<i32>(manager, m.offset(g.current_state)) };
    let (kind, digit) = match state {
        PASS_AUTHENTICATION => (g.pass_type, g.pass_digit),
        PASS_REGIST => (g.regist_type, g.regist_digit),
        _ => return,
    };
    let mut last = LAST.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < DIGIT_EVERY) {
        return;
    }
    let mut args = [kind];
    let Some(screen) = m.invoke(g.find_object, std::ptr::null_mut(), &mut args).filter(|o| !o.is_null()) else { return };
    let count = if state == PASS_AUTHENTICATION {
        typed(m, screen, g.pass_input)
    } else {
        match unsafe { read::<i32>(screen as *const u8, m.offset(g.regist_message)) } {
            INPUT => typed(m, screen, g.regist_input),
            INPUT_CONFIRM => typed(m, screen, g.regist_confirm),
            _ => None,
        }
    };
    let Some(count) = count.filter(|&c| c < 4) else { return };
    let mut value = (pin[count] - b'0') as i32;
    let mut args = [&mut value as *mut i32 as P];
    m.invoke(digit, screen, &mut args);
    *last = Some(std::time::Instant::now());
}

/// The game's save of the player, as sent: applied to the card's record.
fn save() {
    let (Some(m), Some(g)) = (mono(), game()) else { return };
    let mut session = SESSION.lock().unwrap();
    let Some(s) = session.as_mut() else { return };
    let (Some(pin), Some(record)) = (s.pin, s.record.as_mut()) else { return };
    let Some(data) = m.invoke(g.create_save, std::ptr::null_mut(), &mut []).filter(|o| !o.is_null()) else {
        log!("bbg: the game's save not made");
        return;
    };
    let mut buffer = vec![0u8; SAVE_SIZE];
    let mut at = buffer.as_mut_ptr() as isize;
    let mut delete_old = false;
    let mut args = [data, &mut at as *mut isize as P, &mut delete_old as *mut bool as P];
    if m.invoke(g.to_ptr, std::ptr::null_mut(), &mut args).is_none() {
        log!("bbg: the game's save not marshalled");
        return;
    }
    apply(m, g, record, &buffer);
    store(&s.card, pin, record);
}

fn get<T: Copy>(b: &[u8], at: usize) -> T {
    unsafe { read::<T>(b.as_ptr(), at) }
}

fn put<T: Copy>(b: &mut [u8], at: usize, value: T) {
    unsafe { write::<T>(b.as_mut_ptr(), at, value) }
}

/// A save applied to a record: the player's values as they are now, the girls' experience won.
fn apply(m: &Mono, g: &Game, record: &mut [u8], save: &[u8]) {
    put(record, CLASS_EXP, get::<i32>(save, SAVE_CLASS_EXP));
    put(record, TITLE, get::<i32>(save, SAVE_TITLE));
    put(record, LAST_GIRL, get::<i32>(save, SAVE_GIRL));
    put(record, HEADPHONE, get::<i32>(save, SAVE_HEADPHONE));
    put(record, WIN_STREAK, get::<i32>(save, SAVE_WIN_STREAK));
    put(record, PLAY_COUNT, get::<i32>(record, PLAY_COUNT) + 1);
    put(record, GIRLS_OPEN, get::<u64>(record, GIRLS_OPEN) | get::<u64>(save, SAVE_GIRLS_OPEN));
    put(record, GIRLS_SEEN, get::<u64>(record, GIRLS_SEEN) | get::<u64>(save, SAVE_GIRLS_SEEN));
    for i in 0..GIRL_COUNT {
        let from = SAVE_GIRLS + i * SAVE_GIRL_SIZE;
        let id = get::<i32>(save, from);
        if id <= 0 {
            continue;
        }
        // the girl's record, else a free one
        let slot = (0..GIRL_COUNT)
            .map(|j| GIRLS + j * GIRL_SIZE)
            .find(|&at| get::<i16>(record, at) as i32 == id)
            .or_else(|| (0..GIRL_COUNT).map(|j| GIRLS + j * GIRL_SIZE).find(|&at| get::<i16>(record, at) <= 0));
        let Some(to) = slot else { continue };
        put(record, to, id as i16);
        let exp = get::<i32>(record, to + GIRL_EXP).saturating_add(get::<i32>(save, from + SAVE_GIRL_EXP));
        put(record, to + GIRL_EXP, exp);
        let mut arg = exp;
        let mut args = [&mut arg as *mut i32 as P];
        if let Some(rank) = m.invoke(g.get_rank, std::ptr::null_mut(), &mut args).filter(|o| !o.is_null()) {
            put(record, to + GIRL_RANK, unsafe { read::<i32>(rank as *const u8, BOXED) } as i16);
        }
        // chat set: id + 10 buttons; skill deck: id + the 4 skills; dress: 8 ids
        record[to + GIRL_CHAT_SET..to + GIRL_CHAT_SET + 44].copy_from_slice(&save[from + SAVE_GIRL_CHAT_SET..from + SAVE_GIRL_CHAT_SET + 44]);
        put(record, to + GIRL_DECK, get::<i32>(save, from + SAVE_GIRL_DECK));
        for k in 0..4 {
            put(record, to + GIRL_DECK + 4 + k * 4, get::<i32>(save, from + SAVE_GIRL_DECK + 4 + k * 12 + 8));
        }
        record[to + GIRL_DRESS..to + GIRL_DRESS + 32].copy_from_slice(&save[from + SAVE_GIRL_DRESS..from + SAVE_GIRL_DRESS + 32]);
    }
}

/// The session's end (the game over): the next entry reads the card again.
fn end() {
    *SESSION.lock().unwrap() = None;
    STATE.store(NONE, Ordering::Relaxed);
}

// ---- Entry ----

#[unsafe(no_mangle)]
pub extern "C" fn EntryGetCardEntryState() -> i32 {
    update();
    STATE.load(Ordering::Relaxed)
}

/// IcCardReadData: card on the reader, its data read, its type, its id (17 ANSI characters).
/// The card is reported off the reader (the game's end waits for it to leave).
#[unsafe(no_mangle)]
pub extern "C" fn EntryGetCardReadData(data: *mut u8) {
    if data.is_null() {
        return;
    }
    unsafe { std::ptr::write_bytes(data, 0, 20) };
    if let Some(s) = SESSION.lock().unwrap().as_ref() {
        unsafe {
            *data.add(1) = 1;
            *data.add(2) = 1;
            std::ptr::copy_nonoverlapping(s.card.as_ptr(), data.add(3), s.card.len().min(16));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryInitCardReadData() {}

/// The 4-digit PIN of an ANSI string.
fn pin_of(text: *const u8) -> Option<[u8; 4]> {
    if text.is_null() {
        return None;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(text as *const std::ffi::c_char) }.to_bytes();
    (text.len() == 4 && text.iter().all(u8::is_ascii_digit)).then(|| text.try_into().unwrap())
}

static REGISTER_PASS: AtomicI32 = AtomicI32::new(NONE);
static CHECK_PASS: AtomicI32 = AtomicI32::new(NONE);
static REGISTER_PLAYER: AtomicI32 = AtomicI32::new(NONE);

#[unsafe(no_mangle)]
pub extern "C" fn EntrySetPassWordRegister_Request(password: *const u8) {
    let pin = pin_of(password);
    if let Some(s) = SESSION.lock().unwrap().as_mut() {
        s.pin = pin;
    }
    STATE.store(IN_NEW_PLAYER_DATA, Ordering::Relaxed);
    REGISTER_PASS.store(SUCCESS, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn EntrySetPassWordRegister_Status() -> i32 {
    REGISTER_PASS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn EntrySetPassWordCheck_Request(password: *const u8) {
    let ok = SESSION.lock().unwrap().as_ref().is_some_and(|s| s.pin.is_some() && s.pin == pin_of(password));
    if ok {
        STATE.store(END, Ordering::Relaxed);
    }
    CHECK_PASS.store(if ok { SUCCESS } else { ERROR_PASS_NG }, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn EntrySetPassWordCheck_Status() -> i32 {
    CHECK_PASS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn EntrySetPassWordCheck_RetryPassInput() -> bool {
    true
}

/// TakeOverData: no player to take over.
#[unsafe(no_mangle)]
pub extern "C" fn EntryGetTakeOverData(data: *mut u8) {
    if !data.is_null() {
        unsafe { std::ptr::write_bytes(data, 0, 56) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryNameCheck(_name: *const u8) -> bool {
    true
}

/// RegistPlayerData: the name (40 ANSI characters), the class down choice. The player created,
/// an empty record (the game's defaults) with the name.
#[unsafe(no_mangle)]
pub extern "C" fn EntryPlayerDataRegister_Request(data: *const u8) {
    let mut session = SESSION.lock().unwrap();
    let Some(s) = session.as_mut() else {
        REGISTER_PLAYER.store(SUCCESS, Ordering::Relaxed);
        return;
    };
    let mut record = vec![0u8; RECORD_SIZE];
    if !data.is_null() {
        let name = unsafe { std::slice::from_raw_parts(data, NAME_SIZE) };
        let len = name.iter().position(|&b| b == 0).unwrap_or(NAME_SIZE);
        record[NAME..NAME + len].copy_from_slice(&name[..len]);
        log!("bbg: new player {}", String::from_utf8_lossy(&name[..len]));
    }
    // a player id of the card's
    let id = s.card.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) & 0x7fff_ffff;
    put(&mut record, PLAYER_ID, id.max(1) as i32);
    let pin = s.pin.unwrap_or(*b"0000");
    store(&s.card, pin, &record);
    s.pin = Some(pin);
    s.record = Some(record);
    STATE.store(END, Ordering::Relaxed);
    REGISTER_PLAYER.store(SUCCESS, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryPlayerDataRegister_Status() -> i32 {
    REGISTER_PLAYER.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryIsReConsent() -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryReConsent_Request() {}

#[unsafe(no_mangle)]
pub extern "C" fn EntryReConsent_Status() -> i32 {
    SUCCESS
}

/// PlayerLockData: never locked.
#[unsafe(no_mangle)]
pub extern "C" fn EntryGetPlayerLockDate(data: *mut u8) {
    if !data.is_null() {
        unsafe { std::ptr::write_bytes(data, 0, 128) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryGetPlayID() -> u64 {
    SESSION.lock().unwrap().as_ref().and_then(|s| s.record.as_ref()).map_or(0, |r| get::<i32>(r, PLAYER_ID) as u64)
}

/// READ_PLAYER_DATA: the card's record.
#[unsafe(no_mangle)]
pub extern "C" fn EntryGetPlayerData(data: *mut u8) {
    if data.is_null() {
        return;
    }
    match SESSION.lock().unwrap().as_ref().and_then(|s| s.record.as_ref()) {
        Some(record) => unsafe { std::ptr::copy_nonoverlapping(record.as_ptr(), data, RECORD_SIZE) },
        None => unsafe { std::ptr::write_bytes(data, 0, RECORD_SIZE) },
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryGetPlayerDataSubHukubikiData(_event: i32, _index: i32, _item: P) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryGetPlayerCardAuthentication() -> bool {
    SESSION.lock().unwrap().as_ref().is_some_and(|s| s.record.is_some())
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryPlayerSaveEndNotice() {
    save();
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryGetContinueBeforeInitEnd() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryContinueAfterInit() {}

#[unsafe(no_mangle)]
pub extern "C" fn EntryGameOver() {
    end();
}

#[unsafe(no_mangle)]
pub extern "C" fn EntryGameOverSceneEnd() {
    end();
}
