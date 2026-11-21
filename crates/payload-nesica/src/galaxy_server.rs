//! Hoshi to Tsubasa no Paradox's game server (`galaxy.rs`): with a NESiCA, the game logs the
//! card's player in to the server the NESYS certificate names (`httphost=` of its server string,
//! `FNesysServerInfoParser`), loads and saves the player's data there.
//!
//! The server list (`httphost=`, `tcphost=` the matching server, `env=`) goes in the certificate
//! (`nesys::GAME_SERVER`) unless the profile sets `WAL_NESICA_NESYS_HOST`; with
//! `WAL_GALAXY_SERVER=0` there is none: the game plays offline (guest play only).
//!
//! HTTP, `POST /<api>` with a form body (`application/x-www-form-urlencoded`, lists as JSON
//! text), answered JSON with the request's `X-GALAXY-API-ID` echoed. The API as the arcade's
//! server answered it (protocol notes of ArcadeMachinist's StarwingParadox mock server): `version`
//! (the client and data versions), `resource` (the game data tables to download: served from
//! the cabinet's copies in `D:\Saved\ACRSaved\SaveData`), `player/profile/load` (the card's
//! player, created on first use), `player/register`, `player/login`, `player/login_bonus`,
//! `game_data/load`, `game_data/load/mission`, `game_data/save` (each list merged by its keys),
//! `matching/server` (the matching server: `tcphost=`), `matching/match_id/generate`, `ranking/*`
//! (empty rankings); every other request `{"result": 1}`.
//!
//! The matching server (`matching/server`, `tcphost=`): TCP, each message a u32 length (little
//! endian) then a `PbMessage` (the game's `message.proto`, its descriptor in the executable:
//! 1 id, 2 type, then the field numbered as the type with the message). `Ping` answered `Pong`
//! (the game is online only with it: `UOnlineObserverWork`'s WebServer bit), `RequestEntryMatching`
//! `ResponseEntryMatching`, `RequestCancelMatching` `ResponseCancelMatching`; every other request
//! `Response` Ok.
//!
//! Records: `galaxy\<card id>.json` in the D: data folder (the player's fields and lists).
//! `WAL_GALAXY_SERVER_LOG=1` logs each request's body and answer.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use wal_payload_common::json::Json;
use wal_payload_common::log;

/// The HTTP API's port, and the matching server's.
pub(crate) const PORT: u16 = 37705;
pub(crate) const MATCHING_PORT: u16 = 37706;

/// The client and data versions the game reports (its SVN revision).
const VERSION: &str = "70571";

/// The lists of the player's game data, and the fields that identify an entry of each (an entry
/// saved replaces the one with the same keys).
const LISTS: &[(&str, &[&str])] = &[
    ("progresses", &["progress_key"]),
    ("options", &["option_key"]),
    ("missions", &["mission_id"]),
    ("quests", &["quest_id"]),
    ("buddies", &["buddy_id"]),
    ("buddy_skills", &["buddy_id", "skill_id"]),
    ("buddy_win_poses", &["buddy_id", "win_pose_id"]),
    ("emblems", &["emblem_id"]),
    ("emblem_parts", &["part_id"]),
    ("titles", &["title_id"]),
    ("line_colors", &["line_color_id"]),
    ("mecha_sets", &["mecha_set_id"]),
    ("mecha_set_parts", &["mecha_set_id", "part_id"]),
    ("mecha_colors", &["mecha_color_id"]),
    ("weapon_set", &["weapon_set_id"]),
    ("weapon_set_slots", &["weapon_set_id", "slot_id"]),
    ("side_weapons", &["side_weapon_id"]),
    ("cockpit_items", &["cockpit_item_id"]),
    ("symbol_chats", &["symbol_chat_id"]),
    ("symbol_chat_slots", &["slot_id"]),
    ("greetings", &["greeting_id"]),
    ("buddy_greetings", &["buddy_id", "greeting_id"]),
    ("game_moneys", &["game_money_id"]),
    ("present_items", &["present_item_id"]),
];

/// The player's fields at creation (the arcade server's defaults).
const PLAYER_DEFAULTS: &[(&str, &str)] = &[
    ("player_name", "\"ＮｏＮａｍｅ\""),
    ("rank_id", "0"),
    ("rank_id_2on2", "0"),
    ("title_id", "0"),
    ("title_id_2on2", "0"),
    ("buddy_id", "0"),
    ("buddy_intimacy", "0"),
    ("line_color_id", "0"),
    ("ranking_pref_name", "\"東京\""),
    ("last_ranking_pref_name", "\"東京\""),
    ("match_mode_id", "0"),
    ("violation_point", "0"),
    ("emblem_id", "0"),
    ("line_color_id_2on2", "0"),
    ("emblem_id_2on2", "0"),
    ("birth_day", "1"),
    ("birth_month", "1"),
    ("mecha_set_id", "0"),
    ("side_weapon_id", "0"),
    ("mecha_preset_id", "0"),
    ("rank_point", "0"),
    ("max_rank_id", "0"),
    ("rank_point_2on2", "0"),
    ("max_rank_id_2on2", "0"),
];

fn verbose() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("WAL_GALAXY_SERVER_LOG").is_ok_and(|v| v.trim() == "1"))
}

/// Starts the server (once).
pub(crate) fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => {
            log!("galaxy: game server on 127.0.0.1:{PORT}");
            let _ = crate::nesys::GAME_SERVER.set(format!("httphost=127.0.0.1:{PORT},tcphost=127.0.0.1:{MATCHING_PORT},env=production"));
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || {
                        if let Err(e) = serve(stream) {
                            log!("galaxy: game server: {e}");
                        }
                    });
                }
            });
        }
        Err(e) => log!("galaxy: game server: 127.0.0.1:{PORT}: {e}"),
    });
    static MATCHING: std::sync::Once = std::sync::Once::new();
    MATCHING.call_once(start_matching);
}

/// Starts the matching server (once).
fn start_matching() {
    match TcpListener::bind(("127.0.0.1", MATCHING_PORT)) {
        Ok(listener) => {
            log!("galaxy: matching server on 127.0.0.1:{MATCHING_PORT}");
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || {
                        if let Err(e) = matching(stream) {
                            log!("galaxy: matching server: {e}");
                        }
                    });
                }
            });
        }
        Err(e) => log!("galaxy: matching server: 127.0.0.1:{MATCHING_PORT}: {e}"),
    }
}

mod pb {
    pub const RESPONSE: u64 = 100;
    pub const PING: u64 = 102;
    pub const PONG: u64 = 103;
    pub const REQUEST_ENTRY_MATCHING: u64 = 200;
    pub const RESPONSE_ENTRY_MATCHING: u64 = 201;
    pub const REQUEST_CANCEL_MATCHING: u64 = 202;
    pub const RESPONSE_CANCEL_MATCHING: u64 = 203;
}

/// A protobuf message being written.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn varint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(b);
                return;
            }
            self.0.push(b | 0x80);
        }
    }
    /// A varint field (zero: omitted, as proto3 does).
    fn u(mut self, field: u64, v: u64) -> Out {
        if v != 0 {
            self.varint(field << 3);
            self.varint(v);
        }
        self
    }
    fn msg(mut self, field: u64, m: Out) -> Out {
        self.varint(field << 3 | 2);
        self.varint(m.0.len() as u64);
        self.0.extend_from_slice(&m.0);
        self
    }
}

/// A message's varint fields and the bytes of its length delimited ones, in order.
fn pb_fields(mut d: &[u8]) -> Vec<(u64, u64, &[u8])> {
    fn varint(d: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = d.split_first()?;
            *d = rest;
            v |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Some(v);
            }
        }
        None
    }
    let mut out = Vec::new();
    while let Some(tag) = varint(&mut d) {
        let field = tag >> 3;
        match tag & 7 {
            0 => match varint(&mut d) {
                Some(v) => out.push((field, v, &[][..])),
                None => break,
            },
            2 => {
                let Some(len) = varint(&mut d) else { break };
                let len = (len as usize).min(d.len());
                out.push((field, 0, &d[..len]));
                d = &d[len..];
            }
            5 if d.len() >= 4 => d = &d[4..],
            1 if d.len() >= 8 => d = &d[8..],
            _ => break,
        }
    }
    out
}

/// One cabinet's connection: requests answered until it closes.
fn matching(mut stream: TcpStream) -> std::io::Result<()> {
    loop {
        let mut len = [0u8; 4];
        stream.read_exact(&mut len)?;
        let mut message = vec![0u8; u32::from_le_bytes(len) as usize];
        stream.read_exact(&mut message)?;
        let f = pb_fields(&message);
        let field = |n: u64| f.iter().find(|(k, _, _)| *k == n);
        let (id, typ) = (field(1).map_or(0, |x| x.1), field(2).map_or(0, |x| x.1));
        let body = field(typ).map_or(&[][..], |x| x.2);
        let (reply_type, reply) = match typ {
            pb::PING => (pb::PONG, Out::default().u(1, id).u(2, now() * 1000)),
            pb::REQUEST_ENTRY_MATCHING => {
                let b = pb_fields(body);
                let get = |n: u64| b.iter().find(|(k, _, _)| *k == n).map_or(0, |x| x.1);
                log!("galaxy: matching entry: player {} card {} play mode {} stage {}", get(1), get(3), get(7), get(18));
                (pb::RESPONSE_ENTRY_MATCHING, Out::default().u(1, id).u(2, 30))
            }
            pb::REQUEST_CANCEL_MATCHING => (pb::RESPONSE_CANCEL_MATCHING, Out::default().u(1, id)),
            _ => {
                log!("galaxy: matching request type {typ} answered Ok");
                (pb::RESPONSE, Out::default().u(1, id))
            }
        };
        if typ != pb::PING || verbose() {
            log!("galaxy: matching {typ} -> {reply_type}");
        }
        let out = Out::default().u(1, id).u(2, reply_type).msg(reply_type, reply);
        let mut frame = (out.0.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&out.0);
        stream.write_all(&frame)?;
    }
}

/// One request, answered; the connection closed.
fn serve(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut length, mut expect, mut api_id) = (0usize, false, String::from("1"));
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or((&header, ""));
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "expect" => expect = value.trim().eq_ignore_ascii_case("100-continue"),
            "x-galaxy-api-id" => api_id = value.trim().to_string(),
            _ => {}
        }
    }
    let mut out = stream;
    if expect {
        out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let path = target.split('?').next().unwrap_or("").trim_matches('/').to_string();
    let form = form(&String::from_utf8_lossy(&body));
    let (kind, reply) = if method == "GET" && path.starts_with("sw-data/") {
        ("text/csv", data_table(&path["sw-data/".len()..]))
    } else {
        let reply = api(&path, &form);
        if verbose() {
            log!("galaxy: {method} /{path} {} -> {reply}", String::from_utf8_lossy(&body));
        } else {
            log!("galaxy: {method} /{path}");
        }
        ("application/json", reply.to_string().into_bytes())
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nX-GALAXY-API: */*\r\nX-GALAXY-API-ID: {api_id}\r\nConnection: close\r\n\r\n",
        reply.len()
    );
    out.write_all(head.as_bytes())?;
    out.write_all(&reply)?;
    out.flush()
}

/// A form body's fields (JSON text values parsed).
fn form(body: &str) -> Json {
    let mut fields = Json::obj();
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (decode(k), decode(v));
        let value = match v.trim_start().as_bytes().first() {
            Some(b'[' | b'{') => Json::parse(&v).unwrap_or(Json::Str(v)),
            _ => Json::Str(v),
        };
        fields.set(&k, value);
    }
    fields
}

/// A form field's percent and `+` decoding.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(v) => {
                    out.push(v);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A game data table, as the cabinet saved it from the arcade server.
fn data_table(name: &str) -> Vec<u8> {
    let name = name.replace(['/', '\\'], "");
    let path = format!("{}\\Saved\\ACRSaved\\SaveData\\{name}", crate::drive::data_dir());
    std::fs::read(&path).unwrap_or_else(|e| {
        log!("galaxy: {path}: {e}");
        Vec::new()
    })
}

/// The data tables (`resource`): the cabinet's saved copies, served by this server.
fn resource() -> Json {
    let dir = format!("{}\\Saved\\ACRSaved\\SaveData", crate::drive::data_dir());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.to_ascii_lowercase().ends_with(".csv")).collect())
        .unwrap_or_default();
    names.sort();
    let urls = names.iter().map(|n| Json::str(format!("http://127.0.0.1:{PORT}/sw-data/{n}"))).collect::<Vec<_>>();
    Json::obj().with("version", VERSION).with("data", urls)
}

fn ok() -> Json {
    Json::obj().with("result", Json::num(1))
}

fn api(path: &str, f: &Json) -> Json {
    match path {
        "version" => Json::obj().with("client_version", VERSION).with("data_version", VERSION).with("stage_ids", Vec::<Json>::new()),
        "resource" => resource(),
        "player/profile/load" => {
            let card = f.s("nesys_id").to_string();
            let record = with_record(&card, |_| {});
            let id = player_id(&card);
            let mut cards = CARDS.lock().unwrap();
            if !cards.iter().any(|(i, _)| *i == id) {
                cards.push((id, card));
            }
            profile(&record)
        }
        "player/register" => {
            let record = with_player(f, |r| {
                for (k, v) in fields(f) {
                    if k == "player_id" {
                        continue;
                    }
                    if k == "progresses" {
                        merge(r, "progresses", v);
                    } else if let Some(p) = r.get_mut("player") {
                        p.set(&k, v);
                    }
                }
            });
            log!("galaxy: player {} registered ({})", record.get("player").map_or(0, |p| p.u("player_id")), player_field(&record, "player_name"));
            ok()
        }
        "player/login" => {
            let record = with_player(f, |r| {
                let today = today();
                let logins = r.get("logins").cloned().unwrap_or_else(Json::obj);
                let same_day = if logins.s("day") == today { logins.u("same_day") + 1 } else { 1 };
                let days = logins.u("days") + u64::from(logins.s("day") != today);
                r.set("logins", Json::obj().with("day", today).with("same_day", Json::num(same_day)).with("days", Json::num(days)));
            });
            log!("galaxy: player {} logged in", player_field(&record, "player_id"));
            let mut login = profile(&record);
            login.set("greeting_ids", vec![Json::num(1)]);
            login.set("battle_count", Json::num(record.u("battle_count")));
            login.set("burst_match", false);
            login.set("next_burst_begin", "");
            login.set("next_burst_end", "");
            login.set("open_boss_matches", Vec::<Json>::new());
            login.set("next_boss_matches", Vec::<Json>::new());
            login
        }
        "player/login_bonus" => ok().with("login_bonuses", Vec::<Json>::new()).with("update_items", Json::obj()),
        "game_data/load" => game_data(&with_player(f, |_| {})),
        "game_data/load/mission" => Json::obj().with("missions", with_player(f, |_| {}).get("lists").and_then(|l| l.get("missions")).cloned().unwrap_or(Json::Arr(Vec::new()))),
        "game_data/save" => {
            let record = with_player(f, |r| {
                for (k, v) in fields(f) {
                    if k == "player_id" {
                        continue;
                    }
                    if matches!(v, Json::Arr(_)) {
                        merge(r, &k, v);
                    } else if let Some(p) = r.get_mut("player") {
                        p.set(&k, v);
                    }
                }
            });
            ok().with("missions", list(&record, "missions"))
        }
        "battle/record" | "battle/record_2on2" => {
            with_player(f, |r| {
                let n = r.u("battle_count") + 1;
                r.set("battle_count", Json::num(n));
            });
            ok()
        }
        "matching/server" => Json::obj().with("ip_addr", format!("127.0.0.1:{MATCHING_PORT}")),
        "matching/match_id/generate" => Json::obj().with("match_id", Json::num(10000 + now() % 90000)),
        p if p.starts_with("ranking/") => {
            let ranking = Json::obj()
                .with("updated_at", "")
                .with("fixed_term", Json::obj().with("from_at", "").with("to_at", ""))
                .with("records", Vec::<Json>::new());
            match f.get("role_id") {
                Some(role) => ranking.with("role_id", role.clone()),
                None => ranking,
            }
        }
        _ => {
            if !verbose() {
                log!("galaxy: /{path} answered {{\"result\": 1}}");
            }
            ok()
        }
    }
}

/// An object's fields, in order.
fn fields(j: &Json) -> Vec<(String, Json)> {
    match j {
        Json::Obj(f) => f.clone(),
        _ => Vec::new(),
    }
}

/// The player's data (`player/profile/load`): its fields, login counts and progress.
fn profile(record: &Json) -> Json {
    let mut p = record.get("player").cloned().unwrap_or_else(Json::obj);
    let logins = record.get("logins").cloned().unwrap_or_else(Json::obj);
    let same_day = if logins.s("day") == today() { logins.u("same_day") } else { 0 };
    p.set("same_day_login_count", Json::num(same_day));
    p.set("total_login_days", Json::num(logins.u("days")));
    p.set("consecutive_login_days", Json::num(u64::from(same_day > 0)));
    p.set("emblem", emblem());
    p.set("progresses", list(record, "progresses"));
    p.set("last_pref_ranking_order_id", Json::num(0));
    p.set("pref_ranking_top_player_count", Json::num(0));
    p.set("official_player_type_id", Json::num(0));
    p
}

fn emblem() -> Json {
    let part = || {
        Json::obj()
            .with("part_id", Json::num(0))
            .with("offset", vec![Json::num(0), Json::num(0)])
            .with("scale", vec![Json::num(1), Json::num(1)])
            .with("angle", Json::num(0))
    };
    Json::obj().with("outline", part()).with("main_design", part()).with("sub_design", part())
}

/// The player's game data (`game_data/load`).
fn game_data(record: &Json) -> Json {
    let mut d = Json::obj().with("player", profile(record));
    for (name, _) in LISTS {
        d.set(name, list(record, name));
    }
    d.set("violation_point", Json::num(player_field(record, "violation_point").parse::<u64>().unwrap_or(0)));
    d.set("winning_streaks_2on2", Json::num(0));
    d
}

fn list(record: &Json, name: &str) -> Json {
    record.get("lists").and_then(|l| l.get(name)).cloned().unwrap_or(Json::Arr(Vec::new()))
}

/// A list's saved entries merged into the record: an entry replaces the one with the same keys
/// (its fields over the saved ones), else is added.
fn merge(record: &mut Json, name: &str, saved: Json) {
    let keys: &[&str] = LISTS.iter().find(|(n, _)| *n == name).map_or(&[], |(_, k)| k);
    if record.get("lists").is_none() {
        record.set("lists", Json::obj());
    }
    let lists = record.get_mut("lists").expect("lists");
    let mut items = match lists.get(name) {
        Some(Json::Arr(items)) => items.clone(),
        _ => Vec::new(),
    };
    if keys.is_empty() {
        log!("galaxy: game data list {name} (no known keys) replaced");
        items = saved.items().to_vec();
    } else {
        let id = |e: &Json| keys.iter().map(|k| e.get(k).map(|v| v.to_string()).unwrap_or_default()).collect::<Vec<_>>();
        for entry in saved.items() {
            match items.iter_mut().find(|e| id(e) == id(entry)) {
                Some(old) => {
                    for (k, v) in fields(entry) {
                        old.set(&k, v);
                    }
                }
                None => items.push(entry.clone()),
            }
        }
    }
    lists.set(name, Json::Arr(items));
}

fn player_field(record: &Json, key: &str) -> String {
    match record.get("player").and_then(|p| p.get(key)) {
        Some(Json::Str(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

// ---- records ----

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Today (UTC), `YYYY-MM-DD`.
fn today() -> String {
    let days = (now() / 86400) as i64;
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

static LOCK: Mutex<()> = Mutex::new(());

fn record_path(card: &str) -> String {
    let card: String = card.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("{}\\galaxy\\{card}.json", crate::drive::data_dir())
}

/// The player id of a card.
fn player_id(card: &str) -> u64 {
    let digits: String = card.chars().filter(char::is_ascii_digit).collect();
    digits.parse::<u64>().map_or(10000, |n| n % 100_000_000 + 10000)
}

/// The card's record, changed by `f` and saved (created with the defaults on first use).
fn with_record(card: &str, f: impl FnOnce(&mut Json)) -> Json {
    let _guard = LOCK.lock().unwrap();
    let path = record_path(card);
    let mut record = std::fs::read_to_string(&path).ok().and_then(|t| Json::parse(&t)).unwrap_or_else(|| {
        let mut player = Json::obj().with("player_id", Json::num(player_id(card))).with("nesys_id", card);
        for (k, v) in PLAYER_DEFAULTS {
            player.set(k, Json::parse(v).unwrap_or(Json::Null));
        }
        log!("galaxy: new player {} for card {card}", player_id(card));
        Json::obj().with("player", player).with("lists", Json::obj())
    });
    f(&mut record);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, record.to_string()) {
        log!("galaxy: {path}: {e}");
    }
    record
}

/// The record of the request's `player_id` (the cards' records searched for it).
fn with_player(form: &Json, f: impl FnOnce(&mut Json)) -> Json {
    let id: u64 = form.s("player_id").parse().unwrap_or(0);
    let card = CARDS.lock().unwrap().iter().find(|(i, _)| *i == id).map(|(_, c)| c.clone()).or_else(|| find_card(id));
    match card {
        Some(card) => with_record(&card, f),
        None => {
            log!("galaxy: no card for player {id}");
            Json::obj().with("player", Json::obj().with("player_id", Json::num(id)))
        }
    }
}

/// Player ids and their cards (seen this session).
static CARDS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());

fn find_card(id: u64) -> Option<String> {
    let dir = format!("{}\\galaxy", crate::drive::data_dir());
    let card = std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let name = e.file_name().into_string().ok()?;
        let card = name.strip_suffix(".json")?.to_string();
        (player_id(&card) == id).then_some(card)
    })?;
    CARDS.lock().unwrap().push((id, card.clone()));
    Some(card)
}
