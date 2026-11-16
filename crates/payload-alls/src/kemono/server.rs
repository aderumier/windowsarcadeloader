//! Kemono Friends 3 Planet Tours' title server (`paradesv_arcade`, the Aime card players'
//! data), answered in amdaemon's ALL.Net server (`allnet.rs`): the game posts to
//! `<ALL.Net uri>paradesv_arcade/<Command>.do` (the uri `WAL_ALLS_TITLE_URI`). No public server
//! exists for it (ARTEMiS has none): the answers follow the game's own protocol classes
//! (`ParadeA.Protocol`).
//!
//! Requests: `param=<hex>` (form encoded), the JSON request obfuscated by `SGNFW.Http.Connection`:
//! the first 4 bytes of the JSON's MD5 (8 hex digits), the JSON XORed with the MD5 of those 4
//! bytes, then an MD5 of the JSON and the key (32 hex digits, not checked). Answers: plain JSON
//! (`error_code` absent: success).
//!
//! The players are kept in `Y:\kemono\<Aime id>.json`: the game's `UserInfo` document as its
//! `UpdateOrLogoutUser` sent it, given back by `GetUserInfo` / `LoginUser` (none: a new player,
//! the game's user creation and first card print). The nationwide ranking (`ServerSetting`'s
//! `rank`) is the players' best scores (`Y:\kemono\ranking.json`). The server settings
//! themselves are the game's defaults (`set` empty); no Parade-M photos, no print codes.

use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::log;

const DIR: &str = "Y:\\kemono";
/// The settings' date (ms since 1970): never newer than the game's copy after the first fetch.
const SETTING_DATE: i64 = 1_546_300_800_000;
/// `GameInfoManager.LoginState`: a player found, none, logged in (a new player's `LoginUser`:
/// the game then starts with the player it made; `UserNotExist` there sends it back to the
/// attract).
const USER_FOUND: i64 = 0;
const USER_NOT_EXIST: i64 = 1;
const LOGGED_IN: i64 = 126;

/// Held for each request (load, change, save).
static LOCK: Mutex<()> = Mutex::new(());

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// MD5 (RFC 1321).
fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11,
        16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend(bits.to_le_bytes());
    let mut h: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in msg.chunks(64) {
        let w: Vec<u32> = chunk.chunks(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(w[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        h = [h[0].wrapping_add(a), h[1].wrapping_add(b), h[2].wrapping_add(c), h[3].wrapping_add(d)];
    }
    let mut out = [0u8; 16];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// A form value, URL-decoded.
fn form_value(body: &str, key: &str) -> Option<String> {
    let raw = body.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))?;
    let mut out = Vec::new();
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                out.push(raw.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(b'%'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// The request's JSON from its obfuscated `param` (`Connection.Encrypt`).
fn decode_param(param: &str) -> Option<String> {
    let param = param.trim();
    if param.len() < 8 + 32 {
        return None;
    }
    let key = unhex(&param[..8])?;
    let data = unhex(&param[8..param.len() - 32])?;
    let mask = md5(&key);
    let plain: Vec<u8> = data.iter().enumerate().map(|(i, b)| b ^ mask[i % mask.len()]).collect();
    String::from_utf8(plain).ok()
}

fn user_path(id: i64) -> String {
    format!("{DIR}\\{id}.json")
}

/// The player's `UserInfo` document, if kept.
fn load_user(id: i64) -> Option<String> {
    (id > 0).then_some(())?;
    std::fs::read_to_string(user_path(id)).ok().filter(|s| !s.trim().is_empty())
}

/// `ResponseUser`: found (its document) or not (`absent`: the result then).
fn response_user(id: i64, version: i64, absent: i64) -> Json {
    let user = Json::obj().with("inDate", now_ms()).with("elpsdSec", 0i64).with("version", version);
    match load_user(id) {
        Some(doc) => user.with("rslt", USER_FOUND).with("json", doc),
        None => user.with("rslt", absent).with("json", ""),
    }
}

/// The ranking's entries (`RankingUser` documents, one per player: the best score), best first.
fn ranking() -> Vec<Json> {
    let text = std::fs::read_to_string(format!("{DIR}\\ranking.json")).unwrap_or_default();
    let mut entries: Vec<Json> = Json::parse(&text).map(|j| j.items().to_vec()).unwrap_or_default();
    entries.sort_by_key(|e| -e.i("score"));
    entries
}

/// A play's ranking entry (`UpdateOrLogoutUser`'s `rank`) kept when it is the player's best.
fn record_rank(rank: &str) {
    let Some(entry) = Json::parse(rank) else { return };
    let id = entry.i("id");
    if id <= 0 {
        return;
    }
    let mut entries = ranking();
    match entries.iter_mut().find(|e| e.i("id") == id) {
        Some(e) if e.i("score") > entry.i("score") => {
            // the best score kept, the name and favorite friend updated
            e.set("name", entry.s("name"));
            e.set("rank", entry.i("rank"));
            e.set("bfID", entry.i("bfID"));
        }
        Some(e) => *e = entry,
        None => entries.push(entry),
    }
    let _ = std::fs::write(format!("{DIR}\\ranking.json"), Json::Arr(entries).to_string());
}

fn answer(command: &str, req: &Json) -> Json {
    let header = req.get("header").cloned().unwrap_or(Json::obj());
    let uid = header.i("UID");
    let version = header.i("version");
    let base = Json::obj().with("server_time", now_ms());
    match command {
        "ServerSetting" => {
            let users: Vec<Json> = ranking()
                .into_iter()
                .take(100)
                .enumerate()
                .map(|(i, e)| {
                    Json::str(
                        Json::obj()
                            .with("order", i as i64)
                            .with("id", e.i("id"))
                            .with("rank", e.i("rank"))
                            .with("name", e.s("name"))
                            .with("bfID", e.i("bfID"))
                            .with("score", e.i("score"))
                            .to_string(),
                    )
                })
                .collect();
            base.with("rslt", 0i64).with("updDate", SETTING_DATE).with("rank", Json::obj().with("users", users))
        }
        "CheckAlive" => base.with("sendDate", now_ms()).with("updDate", SETTING_DATE),
        "GetUserInfo" => base.with("user", response_user(uid, version, USER_NOT_EXIST)),
        "LoginUser" => base.with("user", response_user(uid, version, LOGGED_IN)),
        "UpdateOrLogoutUser" => {
            let user = req.s("user");
            if uid > 0 && !user.is_empty() {
                let _ = std::fs::write(user_path(uid), user);
                record_rank(req.s("rank"));
                log!("kemono: player {uid} ({}) saved, score {}", req.s("name"), req.i("score"));
            }
            base.with("rslt", 0i64)
        }
        "GetPrintLog" => base.with("rslt", 0i64).with("logs", Json::obj().with("logs", Vec::<Json>::new())),
        "SendLog" | "SendPrintLog" => base.with("rslt", 0i64),
        _ => {
            log!("kemono: command {command} unknown");
            base.with("rslt", 0i64)
        }
    }
}

/// ALL.Net's server: `paradesv_arcade/<Command>.do` the game's, anything else (amdaemon's
/// polling) answered empty.
fn title(path: &str, body: &[u8]) -> Vec<u8> {
    let path = path.split('?').next().unwrap_or("");
    if !path.contains("/paradesv_arcade/") {
        return Vec::new();
    }
    let command = path.rsplit('/').next().unwrap_or("").trim_end_matches(".do").to_string();
    let text = String::from_utf8_lossy(body);
    let json = form_value(&text, "param").and_then(|p| decode_param(&p)).unwrap_or_default();
    let req = Json::parse(&json).unwrap_or(Json::obj());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::create_dir_all(DIR);
    let answer = answer(&command, &req).to_string();
    log!("kemono: {command} {} -> {}", json.chars().take(400).collect::<String>(), answer.chars().take(300).collect::<String>());
    answer.into_bytes()
}

pub(crate) fn init() {
    wal_payload_common::allnet::set_title(title);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_vectors() {
        let hex = |b: [u8; 16]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"The quick brown fox jumps over the lazy dog")), "9e107d9d372bb6826bd81d3542a419d6");
    }

    #[test]
    fn param_round_trip() {
        // Connection.Encrypt's layout
        let json = r#"{"header":{"UID":12}}"#;
        let sum = md5(json.as_bytes());
        let mask = md5(&sum[..4]);
        let mut p: String = sum[..4].iter().map(|b| format!("{b:02x}")).collect();
        p.extend(json.bytes().enumerate().map(|(i, b)| format!("{:02x}", b ^ mask[i % 16])));
        p.push_str(&"0".repeat(32));
        assert_eq!(decode_param(&p).as_deref(), Some(json));
    }
}
