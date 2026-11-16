//! Magician's Dead's game server (`magiciansdead.rs`): with a NESiCA, the game logs the card's
//! player in to the server its NESYS card data names, and saves each play there.
//!
//! The card data (NESYS card select, data type 0) is a status line then JSON: `game_server`
//! (`host:port`) and `user_id`. The payload writes it as the card's saved data
//! (`card_<id>_0.bin` in the D: data folder, `nesys.rs`), pointing at this server.
//!
//! The protocol is the game's `message.proto` (its descriptor is in the executable): TCP, each
//! message a u32 length (little endian) then a `PbMessage` { 1 id, 2 type, then the field
//! numbered as the type with the message }. Answered: `Ping` (`Pong`), `RequestLoginPlayer`
//! (the card's profile: name, play count, last played character and mode, battle record),
//! `RequestPlayerInfo` (characters, money, tutorial and mission progress), `RequestSetFeeMode`,
//! `RequestSetPlayMode` (`Response` Ok), `RequestEntryMatching` (the requested character: the
//! battle runs on the cabinet with computer players; no `NotifyMatchMade`, no dedicated server:
//! the national matches between cabinets are not available) and `RequestSaveResult` (the play
//! saved). Every other request: `Response` Ok.
//!
//! Records: `magiciansdead\<card id>.json` in the D: data folder.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use wal_payload_common::json::Json;
use wal_payload_common::log;

mod pb {
    pub const RESPONSE: u64 = 100;
    pub const PING: u64 = 102;
    pub const PONG: u64 = 103;
    pub const REQUEST_LOGIN_PLAYER: u64 = 200;
    pub const RESPONSE_LOGIN_PLAYER: u64 = 201;
    pub const REQUEST_PLAYER_INFO: u64 = 202;
    pub const RESPONSE_PLAYER_INFO: u64 = 203;
    pub const REQUEST_SET_FEE_MODE: u64 = 204;
    pub const REQUEST_SET_PLAY_MODE: u64 = 206;
    pub const REQUEST_ENTRY_MATCHING: u64 = 208;
    pub const RESPONSE_ENTRY_MATCHING: u64 = 209;
    pub const REQUEST_SAVE_RESULT: u64 = 210;
    pub const RESPONSE_SAVE_RESULT: u64 = 211;
}

// ---- protobuf ----

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
    fn i(self, field: u64, v: i64) -> Out {
        self.u(field, v as u64)
    }
    fn bytes(mut self, field: u64, data: &[u8]) -> Out {
        self.varint(field << 3 | 2);
        self.varint(data.len() as u64);
        self.0.extend_from_slice(data);
        self
    }
    fn s(self, field: u64, s: &str) -> Out {
        if s.is_empty() { self } else { self.bytes(field, s.as_bytes()) }
    }
    fn msg(self, field: u64, m: Out) -> Out {
        self.bytes(field, &m.0)
    }
}

enum Val<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

/// A message's fields in order (unknown wire types end the parse).
fn fields(mut d: &[u8]) -> Vec<(u64, Val<'_>)> {
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
                Some(v) => out.push((field, Val::Int(v))),
                None => break,
            },
            2 => {
                let Some(len) = varint(&mut d) else { break };
                let len = (len as usize).min(d.len());
                out.push((field, Val::Bytes(&d[..len])));
                d = &d[len..];
            }
            5 if d.len() >= 4 => d = &d[4..],
            1 if d.len() >= 8 => d = &d[8..],
            _ => break,
        }
    }
    out
}

fn int(f: &[(u64, Val)], field: u64) -> u64 {
    f.iter().find_map(|(n, v)| match v {
        Val::Int(i) if *n == field => Some(*i),
        _ => None,
    }).unwrap_or(0)
}

fn bytes<'a>(f: &[(u64, Val<'a>)], field: u64) -> &'a [u8] {
    f.iter().find_map(|(n, v)| match v {
        Val::Bytes(b) if *n == field => Some(*b),
        _ => None,
    }).unwrap_or(&[])
}

fn string(f: &[(u64, Val)], field: u64) -> String {
    String::from_utf8_lossy(bytes(f, field)).into_owned()
}

fn all<'a>(f: &[(u64, Val<'a>)], field: u64) -> Vec<&'a [u8]> {
    f.iter().filter_map(|(n, v)| match v {
        Val::Bytes(b) if *n == field => Some(*b),
        _ => None,
    }).collect()
}

// ---- records ----

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn record_path(card: u64) -> String {
    format!("{}\\magiciansdead\\{card}.json", crate::drive::data_dir())
}

fn load(card: u64) -> Json {
    std::fs::read_to_string(record_path(card)).ok().and_then(|t| Json::parse(&t)).unwrap_or_else(Json::obj)
}

fn save(card: u64, record: &Json) {
    let path = record_path(card);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, record.to_string()) {
        log!("magiciansdead: {path}: {e}");
    }
}

/// The player id of a card (`user_id` of its card data).
pub(crate) fn user_id(card: &str) -> u64 {
    card.parse::<u64>().map_or(1, |n| n % 1_000_000_000 + 1)
}

fn player_name() -> String {
    std::env::var("WAL_NESICA_PLAYER_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "PLAYER".into())
}

/// The logged in player of a connection.
#[derive(Default)]
struct Session {
    card: u64,
    id: u64,
    chara: u64,
    play_mode: u64,
}

fn profile(record: &Json, s: &Session, location: u64, location_name: &str, mac: u64) -> Out {
    Out::default()
        .u(1, s.id)
        .u(2, mac)
        .u(3, s.card)
        .s(4, &player_name())
        .u(5, location)
        .s(7, location_name)
        .u(8, record.u("playCount"))
        .i(11, record.i("lastPlayed"))
        .u(12, record.u("lastChara"))
        .u(13, record.u("lastMode"))
}

fn configs(record: &Json, mut out: Out, field: u64) -> Out {
    for c in record.a("configs") {
        let raw: Vec<u8> = (0..c.s("raw").len() / 2).filter_map(|i| u8::from_str_radix(&c.s("raw")[i * 2..i * 2 + 2], 16).ok()).collect();
        let mut m = Out::default().s(1, c.s("name")).s(10, c.s("str")).i(11, c.i("num"));
        if !raw.is_empty() {
            m = m.bytes(12, &raw);
        }
        out = out.msg(field, m);
    }
    out
}

fn character(c: &Json) -> Out {
    Out::default().u(1, c.u("type")).u(2, c.u("rank")).i(7, c.i("totalRankPoint"))
}

fn handle(s: &mut Session, id: i64, typ: u64, body: &[u8]) -> (u64, Out) {
    let f = fields(body);
    let reply_id = id as u64;
    match typ {
        pb::PING => (pb::PONG, Out::default().u(1, reply_id).i(2, now() * 1000)),
        pb::REQUEST_LOGIN_PLAYER => {
            s.id = int(&f, 1);
            s.card = int(&f, 3);
            if s.card == 0 {
                s.card = std::env::var("WAL_NESICA_CARD_ID").ok().and_then(|c| c.parse().ok()).unwrap_or(0);
            }
            let record = load(s.card);
            log!("magiciansdead: player {} (card {}) logged in, {} plays", s.id, s.card, record.u("playCount"));
            let battle = Out::default().msg(
                2,
                Out::default().u(1, record.u("win")).u(2, record.u("lose")).u(3, record.u("draw")).u(4, record.u("timeover")),
            );
            let p = profile(&record, s, int(&f, 5), &string(&f, 6), int(&f, 2));
            (pb::RESPONSE_LOGIN_PLAYER, configs(&record, Out::default().u(1, reply_id).msg(2, p).msg(3, battle), 4))
        }
        pb::REQUEST_PLAYER_INFO => {
            let record = load(s.card);
            let mut out = Out::default().u(1, reply_id);
            for c in record.a("charas") {
                out = out.msg(2, character(c));
            }
            out = configs(&record, out, 3).s(4, "WindowsLoader").u(5, record.u("money")).u(6, record.u("tutorial"));
            for m in record.a("missions") {
                out = out.msg(7, Out::default().s(1, m.s("script")).u(2, m.u("clearRank")));
            }
            (pb::RESPONSE_PLAYER_INFO, out)
        }
        pb::REQUEST_SET_PLAY_MODE => {
            s.play_mode = int(&f, 1);
            (pb::RESPONSE, Out::default().u(1, reply_id))
        }
        pb::REQUEST_SET_FEE_MODE => (pb::RESPONSE, Out::default().u(1, reply_id)),
        pb::REQUEST_ENTRY_MATCHING => {
            s.chara = int(&f, 7);
            let record = load(s.card);
            let saved = record.a("charas").iter().find(|c| c.u("type") == s.chara).cloned().unwrap_or_else(|| Json::obj().with("type", Json::num(s.chara)));
            log!("magiciansdead: entry: stage {} character {} mode {}", int(&f, 6), s.chara, s.play_mode);
            (pb::RESPONSE_ENTRY_MATCHING, Out::default().u(1, reply_id).msg(2, character(&saved)).u(4, 30))
        }
        pb::REQUEST_SAVE_RESULT => {
            save_result(s, &f);
            (pb::RESPONSE_SAVE_RESULT, Out::default().u(1, reply_id))
        }
        _ => {
            log!("magiciansdead: game server request type {typ} answered Ok");
            (pb::RESPONSE, Out::default().u(1, reply_id))
        }
    }
}

fn save_result(s: &Session, f: &[(u64, Val)]) {
    let mut record = load(s.card);
    record.set("playCount", Json::num(record.u("playCount") + 1));
    record.set("lastPlayed", Json::num(now()));
    record.set("lastChara", Json::num(s.chara));
    record.set("lastMode", Json::num(s.play_mode));
    // Match.Result: win, lose, draw, time over
    let result = ["win", "lose", "draw", "timeover"][(int(f, 3) as usize).min(3)];
    record.set(result, Json::num(record.u(result) + 1));
    // the character's rank
    let mut charas: Vec<Json> = record.a("charas").to_vec();
    let rank = int(f, 7);
    let total = int(f, 9) as i64;
    match charas.iter_mut().find(|c| c.u("type") == s.chara) {
        Some(c) => {
            c.set("rank", Json::num(rank.max(c.u("rank"))));
            c.set("totalRankPoint", Json::num(total));
        }
        None => charas.push(Json::obj().with("type", Json::num(s.chara)).with("rank", Json::num(rank)).with("totalRankPoint", Json::num(total))),
    }
    record.set("charas", Json::Arr(charas));
    // the missions cleared (best rank kept)
    let mut missions: Vec<Json> = record.a("missions").to_vec();
    for m in all(f, 15) {
        let m = fields(m);
        let (script, clear) = (string(&m, 1), int(&m, 2));
        match missions.iter_mut().find(|x| x.s("script") == script) {
            Some(x) => x.set("clearRank", Json::num(clear.max(x.u("clearRank")))),
            None => missions.push(Json::obj().with("script", Json::str(script)).with("clearRank", Json::num(clear))),
        }
    }
    record.set("missions", Json::Arr(missions));
    // the tutorials: their play mode number
    if (1..=2).contains(&s.play_mode) {
        record.set("tutorial", Json::num(s.play_mode.max(record.u("tutorial"))));
    }
    save(s.card, &record);
    log!("magiciansdead: play saved (card {}, {result}, mode {})", s.card, s.play_mode);
}

fn serve(mut stream: TcpStream) {
    let mut session = Session::default();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        while buf.len() >= 4 {
            let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            if buf.len() < 4 + len {
                break;
            }
            let msg: Vec<u8> = buf.drain(..4 + len).skip(4).collect();
            let f = fields(&msg);
            let (id, typ) = (int(&f, 1) as i64, int(&f, 2));
            let (reply_type, body) = handle(&mut session, id, typ, bytes(&f, typ));
            let out = Out::default().i(1, id).u(2, reply_type).msg(reply_type, body);
            let mut frame = (out.0.len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(&out.0);
            if stream.write_all(&frame).is_err() {
                return;
            }
        }
    }
}

static PORT: Mutex<u16> = Mutex::new(0);

/// Starts the server and writes the card's data (its NESYS card data pointing at the server).
pub(crate) fn start() {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            log!("magiciansdead: game server not started: {e}");
            return;
        }
    };
    let port = listener.local_addr().map_or(0, |a| a.port());
    *PORT.lock().unwrap() = port;
    let card = crate::rfid::card_id();
    let data = format!("1\n\n{{\"game_server\":\"127.0.0.1:{port}\",\"user_id\":\"{}\"}}", user_id(&card));
    let file = format!("{}\\card_{card}_0.bin", crate::drive::data_dir());
    if let Err(e) = std::fs::write(&file, data) {
        log!("magiciansdead: {file}: {e}");
    }
    log!("magiciansdead: game server on 127.0.0.1:{port}");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_nodelay(true);
            std::thread::spawn(move || serve(stream));
        }
    });
}
