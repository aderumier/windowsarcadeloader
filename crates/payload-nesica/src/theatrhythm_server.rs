//! Theatrhythm's card server (Square Enix's, `card_server` of the NESYS certificate), the
//! payload's own on `127.0.0.1:33702`: plain HTTP (`theatrhythm_mono.rs`), JSON requests and
//! answers (`/api/v1/<group>/<name>`, Unity's `JsonUtility` on the game's side; a failure is
//! `result` `NG` and a `code`: the game reads no body of a non-2xx status).
//!
//! The players' records (`theatrhythm-server/<card id>.json` in the game's folder) hold what
//! the game reports at the end of each credit (`theatrhythm_player.rs`), answered back when the
//! card comes again. `WAL_THEATRHYTHM_SERVER_LOG=1`: every request and answer in the log.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use wal_payload_common::json::Json;
use wal_payload_common::log;

use crate::theatrhythm_player as player;

pub(crate) const PORT: u16 = 33702;
/// The cabinet's id (its MAC address).
pub(crate) const CABINET_ID: &str = "00:11:22:33:44:55";
/// The shop's id (the NESYS certificate's `t_id`).
pub(crate) const LOCATION_ID: u32 = 1;

fn verbose() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("WAL_THEATRHYTHM_SERVER_LOG").is_ok_and(|v| v.trim() == "1"))
}

/// Starts the server (once).
pub(crate) fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => {
            log!("theatrhythm: card server on 127.0.0.1:{PORT}");
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || {
                        if let Err(e) = serve(stream) {
                            log!("theatrhythm: card server: {e}");
                        }
                    });
                }
            });
        }
        Err(e) => log!("theatrhythm: card server: 127.0.0.1:{PORT}: {e}"),
    });
}

/// One request, answered; the connection closed.
fn serve(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut length, mut expect) = (0usize, false);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or((&header, ""));
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "expect" => expect = value.trim().eq_ignore_ascii_case("100-continue"),
            "gameversion" => player::set_game_version(value.trim()),
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
    let request = if body.trim().is_empty() { Json::obj() } else { Json::parse(&body).unwrap_or_else(Json::obj) };
    let path = path.split('?').next().unwrap_or("");
    let api = path.strip_prefix("/api/v1/").unwrap_or(path);
    let (status, kind, text) = match player::answer(api, &request) {
        player::Answer::Json(answer) => (200, "application/json; charset=utf-8", answer.to_string()),
        player::Answer::Text(text) => (200, "text/plain", text.to_string()),
        player::Answer::NotFound => (404, "text/plain", String::new()),
    };
    if verbose() {
        log!("theatrhythm: {method} {path} {body} -> {status} {text}");
    } else {
        log!("theatrhythm: {method} {path} -> {status}");
    }
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len());
    out.write_all(head.as_bytes())?;
    out.write_all(text.as_bytes())?;
    out.flush()
}

/// The NESYS certificate's information: the shop, its place, the servers.
pub(crate) fn cert_info() -> Json {
    let server = format!("127.0.0.1:{PORT}");
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
        .with("game_server", "127.0.0.1")
        .with("ranking_server", "WAL")
        .with("card_server", server)
        .with("ip_addr", "127.0.0.1")
}

/// NESYS card data of `card`.
pub(crate) fn card_data(card: &str) -> Json {
    player::card_data(card)
}

/// NESYS card insert (the card's first use).
pub(crate) fn card_insert(card: &str) -> Json {
    player::card_insert(card)
}

/// NESYS income notice (game start, end, continue, buy).
pub(crate) fn income(_card: &str, _kind: i32) -> Json {
    Json::obj().with("status", 1).with("error_number", 0)
}
