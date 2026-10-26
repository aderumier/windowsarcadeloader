//! Theatrhythm's `SimpleNesys.dll`: the NESYS client (HTTPS REST to cert2.nesys.jp with the
//! cabinet's client certificate in the cabinet), here answered by itself.
//!
//! Online (default): certified at once (`ONLINE_MODE`), the certificate's servers pointing at
//! the payload's own card server (`theatrhythm_server.rs`, `127.0.0.1:33702`); the card requests
//! (card data, insert, the income notices of each play) answered at once from the card server's
//! records. Three methods of the game's managed code are replaced (Mono's compiled code, at
//! `NESYS_INIT`): `ServerManager.SetProtcol` (the card server is plain HTTP: "https://" ignored),
//! `PhotonManager.Connect` / `Disconnect` (the matching server, Photon: "online", nobody else to
//! match with).
//!
//! `WAL_THEATRHYTHM_SERVER=0`: standalone, as a cabinet without network: guest play only.

use std::ffi::{CStr, CString, c_char};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use wal_payload_common::json::Json;
use wal_payload_common::log;

use crate::theatrhythm_server as server;

type P = *mut std::ffi::c_void;

/// NESYS_STATUS
const PRE_INIT: i32 = 0;
const STANDALONE_MODE: i32 = 1;
const ONLINE_MODE: i32 = 2;
/// NESYS_ERR_STANDALONE_MODE
const ERR_STANDALONE: i32 = -4;
/// NESYS_ERR_INVALID_HANDLE
const ERR_INVALID_HANDLE: i32 = -8;
/// NESYS_MAINTENANCE_TIME_FEW_MINITS: no maintenance message (-1: maintenance now, the card
/// reader off; -3: a "hold" message)
const NO_MAINTENANCE: i32 = -2;
/// NESYS_REQUEST_STATUS
const REQUEST_OK: i32 = 1;
const REQUEST_ERROR: i32 = 2;

static STATUS: AtomicI32 = AtomicI32::new(PRE_INIT);

/// Whether the card server answers (`WAL_THEATRHYTHM_SERVER`, on by default).
pub(crate) fn online() -> bool {
    !std::env::var("WAL_THEATRHYTHM_SERVER").is_ok_and(|v| v.trim() == "0")
}

fn nesys_call(name: &'static str) {
    static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap();
    if !seen.contains(&name) {
        seen.push(name);
        log!("theatrhythm: {name} called");
    }
}

/// The requests' answers (JSON), by handle (from 1), kept until the handle is closed.
static REQUESTS: Mutex<Vec<(i32, CString)>> = Mutex::new(Vec::new());

/// A request answered at once: its handle.
fn request(name: &str, answer: Json) -> i32 {
    static NEXT: AtomicI32 = AtomicI32::new(1);
    let handle = NEXT.fetch_add(1, Ordering::Relaxed);
    let text = answer.to_string();
    log!("theatrhythm: NESYS {name} -> {text}");
    REQUESTS.lock().unwrap().push((handle, CString::new(text).unwrap_or_default()));
    handle
}

fn card(id: *const c_char) -> String {
    if id.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_INIT(game_id: i32, version: i32) -> i32 {
    let online = online();
    log!("theatrhythm: NESYS_INIT game {game_id} version {version}: {}", if online { "online, the payload's card server" } else { "standalone" });
    STATUS.store(STANDALONE_MODE, Ordering::Relaxed);
    if online {
        server::start();
        crate::theatrhythm_mono::patch();
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_RELEASE() -> i32 {
    nesys_call("NESYS_RELEASE");
    STATUS.store(PRE_INIT, Ordering::Relaxed);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_LIBRARY_VER() -> *const u8 {
    c"1.0.0".as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_CERT_NAME() -> *const u8 {
    c"WAL".as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_SERVER_URL() -> *const u8 {
    c"127.0.0.1".as_ptr().cast()
}

/// NESYS_LANINTERFACE: DHCP flag, then gateway, IP, mask, DHCP servers, DNS servers (32 bytes
/// each), MAC address (64 bytes), ANSI strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn NESYS_GET_LANINTERFACE(out: *mut u8) -> i32 {
    nesys_call("NESYS_GET_LANINTERFACE");
    if out.is_null() {
        return -1;
    }
    let mut info = [0u8; 4 + 5 * 32 + 64];
    let fields: [(usize, &str); 6] =
        [(4, "192.168.1.1"), (36, "192.168.1.2"), (68, "255.255.255.0"), (100, ""), (132, "192.168.1.1"), (164, server::CABINET_ID)];
    for (at, s) in fields {
        info[at..at + s.len()].copy_from_slice(s.as_bytes());
    }
    unsafe { std::ptr::copy_nonoverlapping(info.as_ptr(), out, info.len()) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_START_ONLINE_MODE() -> i32 {
    nesys_call("NESYS_START_ONLINE_MODE");
    if !online() {
        return ERR_STANDALONE;
    }
    STATUS.store(ONLINE_MODE, Ordering::Relaxed);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_STOP_ONLINE_MODE() -> i32 {
    nesys_call("NESYS_STOP_ONLINE_MODE");
    STATUS.store(STANDALONE_MODE, Ordering::Relaxed);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_RESTART_ONLINE_MODE() -> i32 {
    nesys_call("NESYS_RESTART_ONLINE_MODE");
    NESYS_START_ONLINE_MODE()
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_UPDATE() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_SET_DEBUG_LOGGER(_logger: P, _kind: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_NETWORK_STATUS() -> i32 {
    STATUS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_NETWORK_ERROR_CODE(_handle: i32) -> u64 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_MAINTENANCE_TIME() -> i32 {
    NO_MAINTENANCE
}

/// The certificate's information: the shop and the servers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn NESYS_GET_CERTINFO(buf: *mut *const c_char, size: *mut i32) -> i32 {
    nesys_call("NESYS_GET_CERTINFO");
    if STATUS.load(Ordering::Relaxed) != ONLINE_MODE {
        return ERR_STANDALONE;
    }
    static INFO: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
    let info = INFO.get_or_init(|| CString::new(server::cert_info().to_string()).unwrap_or_default());
    unsafe {
        if !buf.is_null() {
            *buf = info.as_ptr();
        }
        if !size.is_null() {
            *size = info.as_bytes().len() as i32;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_REQUEST_STATUS(handle: i32) -> i32 {
    if REQUESTS.lock().unwrap().iter().any(|(h, _)| *h == handle) { REQUEST_OK } else { REQUEST_ERROR }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn NESYS_GET_REQUEST_RECIEVED_DATA(handle: i32, buf: *mut *const c_char, size: *mut i32) -> i32 {
    let requests = REQUESTS.lock().unwrap();
    let Some((_, answer)) = requests.iter().find(|(h, _)| *h == handle) else { return ERR_INVALID_HANDLE };
    unsafe {
        if !buf.is_null() {
            *buf = answer.as_ptr();
        }
        if !size.is_null() {
            *size = answer.as_bytes().len() as i32;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_CLOSE_REQUEST_HANDLE(handle: i32) -> i32 {
    REQUESTS.lock().unwrap().retain(|(h, _)| *h != handle);
    0
}

/// NESYS_INCOME_TYPE: game start, end, continue, buy.
#[unsafe(no_mangle)]
pub extern "C" fn NESYS_NOTIFY_INCOME_REQUEST(card_id: *const c_char, coin: i32, kind: i32, _options: P, _count: i32) -> i32 {
    if STATUS.load(Ordering::Relaxed) != ONLINE_MODE {
        return ERR_STANDALONE;
    }
    let card = card(card_id);
    request(&format!("income {kind} ({coin} coins) card {card}"), server::income(&card, kind))
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_CARDDATA_REQUEST(card_id: *const c_char) -> i32 {
    if STATUS.load(Ordering::Relaxed) != ONLINE_MODE {
        return ERR_STANDALONE;
    }
    let card = card(card_id);
    request(&format!("card data {card}"), server::card_data(&card))
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_CARD_INSERT_REQUEST(card_id: *const c_char) -> i32 {
    if STATUS.load(Ordering::Relaxed) != ONLINE_MODE {
        return ERR_STANDALONE;
    }
    let card = card(card_id);
    request(&format!("card insert {card}"), server::card_insert(&card))
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_CARD_REISSUE_REQUEST(card_id: *const c_char) -> i32 {
    if STATUS.load(Ordering::Relaxed) != ONLINE_MODE {
        return ERR_STANDALONE;
    }
    let card = card(card_id);
    request(&format!("card reissue {card}"), Json::obj().with("status", 1).with("error_number", 0).with("message", ""))
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_PLAYLOG_REQUEST(_game: i32, _card: *const c_char, _begin: P, _end: P) -> i32 {
    nesys_call("NESYS_GET_PLAYLOG_REQUEST");
    ERR_STANDALONE
}

type NumberCallback = unsafe extern "system" fn(*const c_char, f64);
type StringCallback = unsafe extern "system" fn(*const c_char, *const u16);
type BoolCallback = unsafe extern "system" fn(*const c_char, i32);

/// Calls back for each of `names` found in `json` (at any depth): its number, string (UTF-16)
/// or boolean.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn NESYS_PARSE_JSON(
    json: *const c_char,
    names: *const *const c_char,
    count: i32,
    number: Option<NumberCallback>,
    string: Option<StringCallback>,
    boolean: Option<BoolCallback>,
) {
    if json.is_null() || names.is_null() || count <= 0 {
        return;
    }
    let text = unsafe { CStr::from_ptr(json) }.to_string_lossy();
    let Some(value) = Json::parse(&text) else {
        log!("theatrhythm: NESYS_PARSE_JSON: not JSON: {text}");
        return;
    };
    let names: Vec<&CStr> = (0..count as usize)
        .filter_map(|i| {
            let p = unsafe { *names.add(i) };
            (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) })
        })
        .collect();
    fn walk(v: &Json, f: &mut dyn FnMut(&str, &Json)) {
        match v {
            Json::Obj(fields) => {
                for (k, v) in fields {
                    f(k, v);
                    walk(v, f);
                }
            }
            Json::Arr(items) => items.iter().for_each(|v| walk(v, f)),
            _ => {}
        }
    }
    walk(&value, &mut |key, v| {
        let Some(name) = names.iter().find(|n| n.to_bytes() == key.as_bytes()) else { return };
        unsafe {
            match v {
                Json::Num(_) => {
                    if let Some(cb) = number {
                        cb(name.as_ptr(), v.as_f64().unwrap_or(0.0));
                    }
                }
                Json::Str(s) => {
                    if let Some(cb) = string {
                        let wide: Vec<u16> = s.encode_utf16().chain([0]).collect();
                        cb(name.as_ptr(), wide.as_ptr());
                    }
                }
                Json::Bool(b) => {
                    if let Some(cb) = boolean {
                        cb(name.as_ptr(), *b as i32);
                    }
                }
                _ => {}
            }
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_START_LOCALSEARCH(_port: u16) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_LOCALSEARCH_ENTRY_NUM() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn NESYS_GET_LOCALSEARCH(_index: i32, _entry: P) -> i32 {
    -1
}
