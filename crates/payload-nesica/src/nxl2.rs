//! NESiCAxLive 2 API: the `NxL2*` exports of `NxL2Core.dll` (system `nesica2`: this DLL is
//! installed under that name, the game imports it).
//!
//! The original library talks to the cabinet's NESiCAxLive 2 system program (shared memory
//! with the game and system settings, a launcher check: without it the game quits at once), to
//! the FastIO driver (inputs, coins) and to the card reader. Here:
//! * inputs: `NxL2GetInput` is player 1's switches of the FastIO input register, as the
//!   original builds it (one player per cabinet): service 0x1, start 0x2, test 0x4, up 0x8,
//!   down 0x10, left 0x20, right 0x40, buttons 1-6 0x80..0x1000 (`fastio::switches`, so
//!   `native_map` applies);
//! * coins: the FastIO coin counter (every player's coin), consumed by `NxL2DecreaseCoinCount`;
//! * settings: `NxL2Get/SetGameSetting*`, `NxL2GetSystemSetting*` by name, from built-in
//!   defaults, `WAL_NXL2_SETTINGS=Name=value,...` (profile), then the values the game saved
//!   (test menu) in `nxl2-settings.txt` of the D: data folder;
//! * card: the `card` virtual input's card (`rfid.rs`, `WAL_NESICA_CARD_ID`) is read when the
//!   game requests one;
//! * no auto end, no store closing time, no network.

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use wal_payload_common::log;

use crate::{drive, fastio, rfid};

/// Million Arthur Arcana Blood's settings (names it reads; volumes 0-100).
const DEFAULTS: &[(&str, i32)] = &[
    // game settings
    ("MasterVolume", 100),
    ("BGMVolume", 100),
    ("SEVolume", 100),
    ("CPULevel", 2),
    ("EventMode", 0),
    ("EventModeCode", 0),
    ("WorldModeCode", 0),
    ("PlayerNameDisp", 1),
    // system settings
    ("IsGameSelectEnable", 0),
    ("Resolution", 0),
    ("StoreMatchGroup", 0),
    ("StoreMatchType", 0),
    ("DemoSoundEnable", 1),
    ("CoinCreditCoin", 1),
    ("CoinCreditCredit", 1),
    ("CloseTimeAnnounceEnable", 0),
];

const SAVE_FILE: &str = "nxl2-settings.txt";

static SETTINGS: Mutex<Option<BTreeMap<String, i32>>> = Mutex::new(None);
static AUTO_END_LOCKS: AtomicI32 = AtomicI32::new(0);

/// Logs the first call of each function (what the game uses).
fn first_call(name: &'static str) {
    static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap();
    if !seen.contains(&name) {
        seen.push(name);
        log!("nxl2: {name} called");
    }
}

fn save_path() -> String {
    format!("{}\\{SAVE_FILE}", drive::data_dir())
}

fn parse_into(map: &mut BTreeMap<String, i32>, text: &str, separator: char) {
    for pair in text.split(separator).map(str::trim).filter(|p| !p.is_empty() && !p.starts_with('#')) {
        if let Some((k, v)) = pair.split_once('=') {
            if let Ok(v) = v.trim().parse() {
                map.insert(k.trim().to_string(), v);
            }
        }
    }
}

fn with_settings<R>(f: impl FnOnce(&mut BTreeMap<String, i32>) -> R) -> R {
    let mut settings = SETTINGS.lock().unwrap();
    let map = settings.get_or_insert_with(|| {
        let mut map: BTreeMap<String, i32> = DEFAULTS.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        if let Ok(spec) = std::env::var("WAL_NXL2_SETTINGS") {
            parse_into(&mut map, &spec, ',');
        }
        if let Ok(saved) = std::fs::read_to_string(save_path()) {
            parse_into(&mut map, &saved, '\n');
        }
        log!("nxl2: settings {map:?}");
        map
    });
    f(map)
}

fn get(name: String, default: i32) -> i32 {
    let v = with_settings(|m| m.get(&name).copied());
    log!("nxl2: setting {name} = {v:?} (default {default})");
    v.unwrap_or(default)
}

fn set(name: String, value: i32) {
    log!("nxl2: set {name} = {value}");
    with_settings(|m| m.insert(name, value));
}

unsafe fn wide(s: *const u16) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *s.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(s, len) })
}

unsafe fn narrow(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2Init(mode: i32) -> i32 {
    first_call("NxL2Init");
    log!("nxl2: init {mode}");
    with_settings(|_| ());
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2Delete() {
    first_call("NxL2Delete");
    log!("nxl2: delete");
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2Update() {
    first_call("NxL2Update");}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetInput() -> u32 {
    first_call("NxL2GetInput");
    // FastIO bits of player 1 (even bits of each pair, test cabinet-wide)
    let s = fastio::switches();
    let mut v = 0;
    for (from, to) in [(0x04, 0x1), (0x10, 0x2), (0x40, 0x4)] {
        if s & from != 0 {
            v |= to;
        }
    }
    // up, down, left, right, buttons 1-6: bits 8, 10 ... 26 -> 0x8 .. 0x1000
    for i in 0..10 {
        if s & (0x100 << (2 * i)) != 0 {
            v |= 0x8 << i;
        }
    }
    static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if LAST.swap(v, Ordering::Relaxed) != v {
        log!("nxl2: input {v:#06x}");
    }
    v
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetCoinCount(_with_pending: i32) -> u32 {
    first_call("NxL2GetCoinCount");
    fastio::coins()
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2DecreaseCoinCount(count: u32) {
    first_call("NxL2DecreaseCoinCount");
    log!("nxl2: {count} coin(s) used");
    fastio::consume_coins(count);
}

/// 0: coin mechanism working.
#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetCoinStatus() -> i32 {
    first_call("NxL2GetCoinStatus");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2NotifyCredit(a: i32, b: i32, c: i32) {
    first_call("NxL2NotifyCredit");
    // called every frame: logged on change
    static LAST: Mutex<(i32, i32, i32)> = Mutex::new((0, 0, 0));
    let mut last = LAST.lock().unwrap();
    if *last != (a, b, c) {
        *last = (a, b, c);
        log!("nxl2: credit {a} {b} {c}");
    }
}

/// 0: I/O board and card reader working.
#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetIOStatus() -> i32 {
    first_call("NxL2GetIOStatus");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2EnableWdt(_timeout: i32) {
    first_call("NxL2EnableWdt");}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetAppicationStep() -> i32 {
    first_call("NxL2GetAppicationStep");
    0
}

/// The game kind of the NESiCAxLive registry key (written by `registry.rs`).
#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetGameID() -> i32 {
    first_call("NxL2GetGameID");
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = "SOFTWARE\\TAITO\\NESiCAxLive\0".encode_utf16().collect();
    let name: Vec<u16> = "GameKind\0".encode_utf16().collect();
    let mut value = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size)
    };
    if r == 0 { value as i32 } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetDllVersion() -> *const c_char {
    first_call("NxL2GetDllVersion");
    c"NxL2Core ver 1.03".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2IsOlderVersion() -> i32 {
    first_call("NxL2IsOlderVersion");
    0
}

/// -1: no closing time announcement.
#[unsafe(no_mangle)]
pub extern "C" fn NxL2CheckStoreOpenTime(_hour: i32, _minute: i32) -> i32 {
    first_call("NxL2CheckStoreOpenTime");
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2NotifyEndApplication(code: i32) {
    first_call("NxL2NotifyEndApplication");
    log!("nxl2: end application {code}");
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2AddBookkeeping(_kind: i32, _data: *const c_void) {
    first_call("NxL2AddBookkeeping");}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2LockAutoEnd() -> i32 {
    first_call("NxL2LockAutoEnd");
    AUTO_END_LOCKS.fetch_add(1, Ordering::Relaxed) + 1
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2UnlockAutoEnd() -> i32 {
    first_call("NxL2UnlockAutoEnd");
    (AUTO_END_LOCKS.fetch_sub(1, Ordering::Relaxed) - 1).max(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetAutoEndLockCount() -> i32 {
    first_call("NxL2GetAutoEndLockCount");
    AUTO_END_LOCKS.load(Ordering::Relaxed)
}

/// -1: the cabinet never ends the game by itself (0 made the game quit at boot).
#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetAutoEndRemainderTime() -> i32 {
    first_call("NxL2GetAutoEndRemainderTime");
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn NxL2GetGameSettingW(name: *const u16, default: i32) -> i32 {
    first_call("NxL2GetGameSettingW");
    get(unsafe { wide(name) }, default)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn NxL2GetGameSettingA(name: *const c_char, default: i32) -> i32 {
    first_call("NxL2GetGameSettingA");
    get(unsafe { narrow(name) }, default)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn NxL2GetSystemSettingW(name: *const u16, default: i32) -> i32 {
    first_call("NxL2GetSystemSettingW");
    get(unsafe { wide(name) }, default)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn NxL2GetSystemSettingA(name: *const c_char, default: i32) -> i32 {
    first_call("NxL2GetSystemSettingA");
    get(unsafe { narrow(name) }, default)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn NxL2SetGameSettingW(name: *const u16, value: i32) {
    first_call("NxL2SetGameSettingW");
    set(unsafe { wide(name) }, value);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn NxL2SetGameSettingA(name: *const c_char, value: i32) {
    first_call("NxL2SetGameSettingA");
    set(unsafe { narrow(name) }, value);
}

/// Saves the game settings (the system settings are the profile's).
#[unsafe(no_mangle)]
pub extern "C" fn NxL2UpdateGameSetting() {
    first_call("NxL2UpdateGameSetting");
    let text: String = with_settings(|m| m.iter().map(|(k, v)| format!("{k}={v}\n")).collect());
    if let Err(e) = std::fs::write(save_path(), text) {
        log!("nxl2: {}: {e}", save_path());
    }
}

/// Card reading: started by the game, complete once a card is in the reader.
#[unsafe(no_mangle)]
pub extern "C" fn NxL2RequestRFID() -> i32 {
    first_call("NxL2RequestRFID");
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2CancelRequestRFID() -> i32 {
    first_call("NxL2CancelRequestRFID");
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn NxL2IsRFIDRead() -> i32 {
    first_call("NxL2IsRFIDRead");
    rfid::card_inserted() as i32
}

/// 0: card read.
#[unsafe(no_mangle)]
pub extern "C" fn NxL2GetRFIDResult() -> i32 {
    first_call("NxL2GetRFIDResult");
    if rfid::card_inserted() { 0 } else { 1 }
}

/// The card id: 16 characters + NUL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn NxL2GetRFIDID(out: *mut u8) -> i32 {
    first_call("NxL2GetRFIDID");
    if out.is_null() {
        return 0;
    }
    unsafe { *out = 0 };
    if !rfid::card_inserted() {
        return 0;
    }
    let mut id = [b'0'; 16];
    for (d, c) in id.iter_mut().zip(rfid::card_id().bytes()) {
        *d = c;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(id.as_ptr(), out, 16);
        *out.add(16) = 0;
    }
    log!("nxl2: card {}", String::from_utf8_lossy(&id));
    1
}
