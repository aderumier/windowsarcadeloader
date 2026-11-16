//! Tetote X Connect's title server (Square Enix's `game.tetoteconnect.jp`, the NESYS
//! certificate's `game_server`), the payload's own on `127.0.0.1:37703` (out of the launchers'
//! input ports, 33700 and up): plain HTTP, JSON
//! (Utf8Json on the game's side; the game reads no body of a non-2xx status).
//!
//! A card's player logs in (`login`: the card id, an access token back, sent with every later
//! request) and loads its save (`GET savedata`); the game saves what changed at each step
//! (`POST savedata`, `tutorial/end`, `game/end`, `purchase`: the save's changed parts, its
//! records, partners, missions, events and inventories by key, the rest whole), kept in
//! `tetote/players/<card id>.json` of the game's folder. A card never seen gets an empty save:
//! the game makes the new player's own and asks for a name. Guest play (no card) logs in too,
//! saving nothing. No news, rankings or matching with other cabinets.
//! `WAL_TETOTE_SERVER_LOG=1`: every request and answer in the log.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

pub(crate) const PORT: u16 = 37703;
/// The shop's id (the NESYS certificate's `t_id`).
const LOCATION_ID: u32 = 1;
/// The access token's prefix (then the card id; a guest's: none).
const TOKEN: &str = "wal-";
/// A token's lifetime (seconds; the game refreshes it at 70%).
const EXPIRES: u32 = 86400;

/// The players' saves are read and written one request at a time.
static SAVES: Mutex<()> = Mutex::new(());

fn verbose() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("WAL_TETOTE_SERVER_LOG").is_ok_and(|v| v.trim() == "1"))
}

/// Starts the server (once).
pub(crate) fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => {
            log!("tetote: title server on 127.0.0.1:{PORT}");
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || {
                        if let Err(e) = serve(stream) {
                            log!("tetote: title server: {e}");
                        }
                    });
                }
            });
        }
        Err(e) => log!("tetote: title server: 127.0.0.1:{PORT}: {e}"),
    });
}

/// The NESYS certificate's information: the shop, its place, the servers.
pub(crate) fn cert_info() -> Json {
    let server = format!("http://127.0.0.1:{PORT}/");
    Json::obj()
        .with("status", 1)
        .with("error_number", 0)
        .with("user_id", 1)
        .with("t_id", LOCATION_ID)
        .with("t_name", "WAL")
        .with("country_id", 1)
        .with("country_name", "日本")
        .with("pref_id", 13)
        .with("t_pref", "東京都")
        .with("t_city", "")
        .with("t_addr", "")
        .with("lat", 35.68)
        .with("lng", 139.76)
        .with("game_server", server.as_str())
        .with("ranking_server", server.as_str())
        .with("card_server", server.as_str())
        .with("ip_addr", "127.0.0.1")
}

/// A request: method, path (without its query and slashes), the card of its token, the
/// client's version, body.
struct Request {
    method: String,
    path: String,
    card: Option<String>,
    version: String,
    body: Json,
}

/// One request, answered; the connection closed.
fn serve(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut length, mut expect, mut card, mut version) = (0usize, false, None, String::new());
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or((&header, ""));
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0),
            "expect" => expect = value.eq_ignore_ascii_case("100-continue"),
            "authorization" => card = value.strip_prefix("Bearer ").and_then(|t| t.strip_prefix(TOKEN)).filter(|c| !c.is_empty()).map(str::to_string),
            "user-agent" => version = value.strip_prefix("VisionClient/").unwrap_or("").to_string(),
            _ => {}
        }
    }
    let mut out = stream;
    if expect {
        out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body);
    let path = target.split('?').next().unwrap_or("").trim_matches('/').to_string();
    let request = Request {
        method: method.clone(),
        path: path.clone(),
        card,
        version,
        body: if body.trim().is_empty() { Json::obj() } else { Json::parse(&body).unwrap_or_else(Json::obj) },
    };
    let text = answer(&request).to_string();
    if verbose() {
        log!("tetote: {method} {target} {body} -> {text}");
    } else if !matches!(path.as_str(), "env" | "refresh") {
        log!("tetote: {method} {target}");
    }
    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len());
    out.write_all(head.as_bytes())?;
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// An empty page of a list (news, rankings).
fn empty_page(per_page: u32) -> Json {
    Json::obj()
        .with("total", 0)
        .with("perPage", per_page)
        .with("currentPage", 1)
        .with("lastPage", 1)
        .with("from", Json::Null)
        .with("to", Json::Null)
        .with("data", Json::Arr(Vec::new()))
}

fn answer(req: &Request) -> Json {
    match (req.method.as_str(), req.path.as_str()) {
        // the latest client: this one (no update to wait for); the masters the game's own
        ("GET", "env") => Json::obj().with("masterVersionId", 0).with("clientVersion", req.version.as_str()),
        ("POST", "login") => {
            let card = req.body.s("cardId");
            if !card.is_empty() {
                log!("tetote: card {card} logged in");
            }
            Json::obj().with("accessToken", format!("{TOKEN}{card}")).with("tokenType", "Bearer").with("expiresIn", EXPIRES)
        }
        ("POST", "refresh") => {
            let token = format!("{TOKEN}{}", req.card.as_deref().unwrap_or(""));
            Json::obj().with("accessToken", token).with("tokenType", "Bearer").with("expiresIn", EXPIRES)
        }
        ("GET", "savedata") => Json::obj().with("savedata", req.card.as_deref().and_then(load).unwrap_or_else(Json::obj)),
        ("DELETE", "savedata") => {
            if let Some(path) = req.card.as_deref().and_then(save_path) {
                let _ = std::fs::remove_file(path);
            }
            Json::obj()
        }
        ("POST", "savedata" | "tutorial/end" | "game/end" | "multi/game/end" | "multibattle/game/end" | "purchase") => {
            if let (Some(card), Some(diff)) = (req.card.as_deref(), req.body.get("savedata")) {
                save(card, diff);
            }
            if let Some(result) = req.body.get("result") {
                log!("tetote: {} {} score {} combo {}", req.path, result.s("chartInfoId"), result.i("score"), result.i("maxCombo"));
            }
            Json::obj()
        }
        // every player name accepted
        ("POST", "ngwords") => Json::obj().with("result", true),
        ("POST", "multi/start") => Json::obj().with("multiplayId", 1),
        ("POST", "multibattle/start") => Json::obj().with("multibattleId", 1),
        ("GET", "news") => empty_page(100),
        ("GET", p) if p.starts_with("rankings/") && p.ends_with("/me") => Json::obj().with("rank", Json::Null).with("score", Json::Null),
        ("GET", p) if p.starts_with("rankings/") => empty_page(5).with("updatedAt", ""),
        ("POST", "init" | "logout" | "logs") => Json::obj(),
        (method, path) => {
            log!("tetote: {method} {path} not known: answered empty");
            Json::obj()
        }
    }
}

fn save_path(card: &str) -> Option<std::path::PathBuf> {
    let valid = !card.is_empty() && card.len() <= 32 && card.chars().all(|c| c.is_ascii_alphanumeric());
    let mut buf = [0u16; 520];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    valid.then(|| exe.parent().unwrap_or(std::path::Path::new(".")).join("tetote").join("players").join(format!("{card}.json")))
}

/// The card's save, if it has one.
fn load(card: &str) -> Option<Json> {
    let _lock = SAVES.lock().unwrap();
    let save = std::fs::read_to_string(save_path(card)?).ok().and_then(|t| Json::parse(&t));
    log!("tetote: card {card}: {}", if save.is_some() { "save loaded" } else { "new player" });
    save
}

/// The save's lists of keyed entries (section, list, the entries' key): the game sends their
/// changed entries only.
const KEYED: &[(&str, &str, &str)] = &[
    ("partnerData", "partners", "characterInfoId"),
    ("missionProgressData", "progresses", "missionInfoId"),
    ("eventProgressData", "progresses", "eventId"),
    ("accessoryInventoryData", "contents", "id"),
    ("costumeInventoryData", "contents", "id"),
    ("degreeInventoryData", "contents", "id"),
    ("coinInventoryData", "contents", "id"),
    ("iconInventoryData", "contents", "id"),
];

/// The save's changed parts (`diff`) laid over the card's save.
fn save(card: &str, diff: &Json) {
    let Some(path) = save_path(card) else { return };
    let _lock = SAVES.lock().unwrap();
    let mut save = std::fs::read_to_string(&path).ok().and_then(|t| Json::parse(&t)).filter(|j| matches!(j, Json::Obj(_))).unwrap_or_else(Json::obj);
    let Json::Obj(sections) = diff else { return };
    for (name, section) in sections {
        let keyed = KEYED.iter().find(|k| k.0 == name);
        let merged = match save.get(name) {
            // the music records: by song chart (an object of them)
            Some(old) if name == "musicRecordData" => {
                let mut new = section.clone();
                let mut records = old.get("records").cloned().unwrap_or_else(Json::obj);
                if let Some(Json::Obj(changed)) = section.get("records") {
                    for (k, v) in changed {
                        records.set(k, v.clone());
                    }
                }
                new.set("records", records);
                new
            }
            Some(old) if let Some(&(_, list, key)) = keyed => {
                let mut entries: Vec<Json> = old.a(list).to_vec();
                for entry in section.a(list) {
                    let id = entry.get(key);
                    match entries.iter_mut().find(|e| e.get(key) == id) {
                        Some(e) => *e = entry.clone(),
                        None => entries.push(entry.clone()),
                    }
                }
                let mut new = section.clone();
                new.set(list, Json::Arr(entries));
                new
            }
            _ => section.clone(),
        };
        save.set(name, merged);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, save.to_string()) {
        Ok(()) => log!("tetote: card {card}: saved"),
        Err(e) => log!("tetote: {}: {e}", path.display()),
    }
}
