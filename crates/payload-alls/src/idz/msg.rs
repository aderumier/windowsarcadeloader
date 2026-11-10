//! The userdb messages: each request's payload (after the 0x30-byte header) starts with its
//! u16 code, its length fixed by the code ([`request_len`]); each answer starts with its own
//! code, its size fixed too. Layouts and logic from minime v005's `src/idz` (`decoder/`,
//! `encoder/`, `handler/`), the codes checked against ARTEMiS's `titles/idz/handlers` (version
//! index 1, v1.30). Version 1.30 uses `loadProfile3` (0x012f), `saveProfile3` (0x0138),
//! `saveTimeAttack2` (0x0136), `loadTopTen2` (0x012c), `load2on2_v2` (0x0132),
//! `loadGeneralReward2` (0x013b), `updateStoryClearNum2` (0x013d), `saveExpedition2` (0x013f);
//! the earlier versions' variants are framed and answered (the profile load / save of v1.10
//! with a generic answer: not ported).

use wal_payload_common::json::Json;
use wal_payload_common::log;

use super::store::{self, Store, arr, bytes, hex, obj, unhex};

mod v2;

/// The store name in the top ten's rows.
const STORE_NAME: &[u8] = b"WindowsArcadeLoader";

/// Request payload lengths by code (minime's `msgLen`s, header excluded).
const REQUESTS: [(u16, usize, &str); 51] = [
    (0x0004, 0x0050, "loadConfig"),
    (0x0006, 0x0020, "loadServerList"),
    (0x0066, 0x00c0, "createProfile"),
    (0x0067, 0x0020, "loadProfile2"),
    (0x0068, 0x0940, "saveProfile2"),
    (0x0069, 0x0020, "lockProfile"),
    (0x006b, 0x0010, "discoverProfile"),
    (0x006d, 0x0020, "lockProfileExtend"),
    (0x006f, 0x0020, "unlockProfile"),
    (0x0071, 0x0050, "createTeam"),
    (0x0073, 0x0010, "updateTeamMember"),
    (0x0077, 0x0010, "loadTeam"),
    (0x0079, 0x0090, "saveNewCar"),
    (0x007b, 0x0010, "createAutoTeam"),
    (0x007f, 0x0010, "updateStoryClearNum1"),
    (0x0081, 0x0010, "updateTeamPoints"),
    (0x0082, 0x0010, "updateProvisionalStoreRank"),
    (0x0084, 0x0410, "updateUiReport"),
    (0x0086, 0x0010, "loadRewardTable"),
    (0x0089, 0x0010, "saveTeamBanner"),
    (0x008a, 0x0020, "updateTeamLeader"),
    (0x008c, 0x0010, "saveExpedition1"),
    (0x008e, 0x0090, "saveGarage"),
    (0x0090, 0x0010, "loadGarage"),
    (0x009a, 0x0010, "saveTopic"),
    (0x009c, 0x0010, "loadGeneralReward1"),
    (0x00a0, 0x0010, "loadGhost"),
    (0x00a2, 0x0040, "checkTeamName"),
    (0x00a5, 0x0020, "saveSettings"),
    (0x00a6, 0x00c0, "saveStocker"),
    (0x00a7, 0x0010, "loadStocker"),
    (0x00a9, 0x0010, "lockGarage"),
    (0x00ab, 0x0010, "loadConfig2"),
    // minime v016: 0x0620 (v005's 0x061c is not a whole number of blocks)
    (0x00ad, 0x0620, "msg00AD"),
    (0x00b0, 0x0010, "load2on2_v1"),
    (0x00b5, 0x00e0, "loadTopTen1"),
    (0x00b9, 0x0010, "loadTeamRanking"),
    (0x00bb, 0x0010, "loadTeamRanking2"),
    (0x00bd, 0x0050, "updateUserLog"),
    (0x00be, 0x0010, "loadEventInfo"),
    (0x00c1, 0x0010, "loadGacha"),
    (0x00cc, 0x0030, "updateResult"),
    (0x00cd, 0x0080, "saveTimeAttack1"),
    (0x012c, 0x0110, "loadTopTen2"),
    (0x012f, 0x0020, "loadProfile3"),
    (0x0132, 0x0010, "load2on2_v2"),
    (0x0136, 0x0080, "saveTimeAttack2"),
    (0x0138, 0x0a70, "saveProfile3"),
    (0x013b, 0x0010, "loadGeneralReward2"),
    (0x013d, 0x0010, "updateStoryClearNum2"),
    (0x013f, 0x0010, "saveExpedition2"),
];

/// The header's protocol: Ver.1 ("110", "130": [`REQUESTS`]), Ver.2 ("210", "230" and later:
/// `v2.rs`, [`v2::REQUESTS`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Proto {
    V1,
    V210,
    V230,
}

impl Proto {
    pub(super) fn of(version: &str) -> Proto {
        match version.trim().parse::<u32>() {
            Ok(v) if v >= 230 => Proto::V230,
            Ok(v) if v >= 200 => Proto::V210,
            _ => Proto::V1,
        }
    }

    pub(super) fn v2(self) -> bool {
        self != Proto::V1
    }
}

/// A request's payload length (its code included) and whether that length is known for sure
/// (else the framer takes what the game sent before it waits), `None` for an unknown code.
pub(super) fn request_info(proto: Proto, code: u16) -> Option<(usize, bool)> {
    if proto.v2() {
        return v2::request_info(proto, code);
    }
    REQUESTS.iter().find(|r| r.0 == code).map(|r| (r.1, true))
}

pub(super) fn name(proto: Proto, code: u16) -> &'static str {
    if proto.v2() {
        return v2::name(code);
    }
    REQUESTS.iter().find(|r| r.0 == code).map_or("unknown", |r| r.2)
}

// ── bytes ──────────────────────────────────────────────────────────────────────────────

pub(super) fn r8(p: &[u8], off: usize) -> u32 {
    p.get(off).copied().unwrap_or(0) as u32
}

pub(super) fn r16(p: &[u8], off: usize) -> u32 {
    r8(p, off) | r8(p, off + 1) << 8
}

pub(super) fn r32(p: &[u8], off: usize) -> u32 {
    r16(p, off) | r16(p, off + 2) << 16
}

/// `p[from..to]` (zeros past `p`'s end).
fn slice(p: &[u8], from: usize, to: usize) -> Vec<u8> {
    (from..to).map(|i| p.get(i).copied().unwrap_or(0)).collect()
}

/// A C string's bytes in `p[from..to]` (up to its NUL).
fn cstr(p: &[u8], from: usize, to: usize) -> Vec<u8> {
    let s = slice(p, from, to);
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    s[..end].to_vec()
}

/// An answer being built (zeroed, its code first).
pub(super) struct Out(Vec<u8>);

impl Out {
    pub(super) fn new(size: usize, code: u16) -> Out {
        let mut out = Out(vec![0; size]);
        out.u16(0, code as u32);
        out
    }

    fn u8(&mut self, off: usize, v: u32) {
        self.0[off] = v as u8;
    }

    fn u16(&mut self, off: usize, v: u32) {
        self.0[off..off + 2].copy_from_slice(&(v as u16).to_le_bytes());
    }

    fn u32(&mut self, off: usize, v: u32) {
        self.0[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn put(&mut self, off: usize, b: &[u8]) {
        self.0[off..off + b.len()].copy_from_slice(b);
    }

    /// A string of at most `max` bytes (NUL ended when it fits).
    fn text(&mut self, off: usize, b: &[u8], max: usize) {
        let n = b.len().min(max);
        self.put(off, &b[..n]);
    }

    fn fill(&mut self, from: usize, to: usize, v: u8) {
        self.0[from..to].fill(v);
    }

    pub(super) fn done(self) -> Vec<u8> {
        self.0
    }
}

/// minime's `generic_res` (0x0001): a status.
pub(super) fn generic(status: u32) -> Vec<u8> {
    let mut out = Out::new(0x20, 0x0001);
    out.u32(4, status);
    out.done()
}

/// An answer of `size` bytes with only its code (the messages minime answers empty).
fn empty(size: usize, code: u16) -> Vec<u8> {
    Out::new(size, code).done()
}

/// The printable part of a Shift-JIS name (the log).
fn printable(name: &[u8]) -> String {
    name.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '?' }).collect()
}

// ── the profile's parts ───────────────────────────────────────────────────────────────

/// A car's 0x60 bytes: its selector (u16 at 0x44) the key; minime's decode / encode keeps
/// every byte.
const CAR_LEN: usize = 0x60;

fn car_selector(car: &[u8]) -> u32 {
    r16(car, 0x44)
}

/// Saves `car` over the player's car with its selector.
pub(super) fn put_car(doc: &mut Json, car: &[u8]) {
    let sel = car_selector(car);
    let cars = arr(doc, "cars");
    let item = Json::obj().with("selector", sel).with("data", hex(car));
    match cars.iter_mut().find(|c| c.u("selector") == sel as u64) {
        Some(c) => *c = item,
        None => cars.push(item),
    }
}

/// The player's selected car (the first one if it is gone).
fn selected_car(doc: &Json) -> Vec<u8> {
    let cars = doc.a("cars");
    let sel = doc.u("car");
    cars.iter().find(|c| c.u("selector") == sel).or(cars.first()).map_or(vec![0; CAR_LEN], |c| bytes(c, "data", CAR_LEN))
}

/// The chara (20 bytes): minime keeps the gender as a byte, the title (request's 0x10) and
/// background (0x12) apart, swapped on load.
fn chara(raw: &[u8]) -> Json {
    let mut head = slice(raw, 0, 0x10);
    head[0] = u8::from(r16(raw, 0) != 0);
    head[1] = 0;
    Json::obj().with("head", hex(&head)).with("title", r16(raw, 0x10)).with("background", r16(raw, 0x12))
}

fn chara_bytes(doc: &Json) -> Vec<u8> {
    let c = doc.get("chara").cloned().unwrap_or(Json::obj());
    let mut out = bytes(&c, "head", 0x10);
    out.extend_from_slice(&(c.u("background") as u16).to_le_bytes());
    out.extend_from_slice(&(c.u("title") as u16).to_le_bytes());
    out
}

/// A mission state (5 grids of 9 cells in 3 bit slices of u16, 0x0c bytes apart) as minime
/// encodes it (0x24 bytes: the grids' 9 bits kept).
fn mission(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 0x24];
    for s in 0..3 {
        for g in 0..5 {
            let off = 0x0c * s + 2 * g;
            let v = (r16(raw, off) & 0x1ff) as u16;
            out[off..off + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// The bitmaps (titles, backgrounds) only gain bits.
fn merge_bits(doc: &mut Json, key: &str, new: &[u8]) {
    let mut bits = bytes(doc, key, new.len());
    for (b, n) in bits.iter_mut().zip(new) {
        *b |= n;
    }
    doc.set(key, hex(&bits));
}

const TITLES_LEN: usize = 0xb4;
const BACKGROUNDS_LEN: usize = 0x24;
const STORY_ROWS: usize = 27;
const COURSES: usize = 20;
const ROUTES: u32 = 40;

/// `createProfile`'s new player (minime's defaults).
pub(super) fn new_profile(p: &[u8], now: u64) -> Json {
    let aime = r32(p, 4);
    let car = slice(p, 0x40, 0xa0);
    let mut doc = Json::obj()
        .with("aime_id", aime)
        .with("name", hex(&cstr(p, 0x1e, 0x34)))
        .with("lv", 1u32)
        .with("exp", 0u32)
        .with("fame", 0u32)
        .with("dpoint", 0u32)
        .with("mileage", 0u32)
        .with("access_time", now)
        .with("register_time", now)
        .with("chara", chara(&slice(p, 0xa0, 0xb4)))
        .with("car", car_selector(&car))
        .with("settings", Json::obj().with("music", 0u32).with("pack", 13640u32).with("paper_cup", 0u32).with("gauges", 5u32))
        .with("unlocks", Json::obj().with("cup", 0u32).with("gauges", 1u32 << 5).with("music", 0u32).with("last_mileage_reward", 0u32))
        .with("tickets", Json::obj())
        .with("missions", Json::obj().with("team", hex(&[0; 0x24])).with("solo", hex(&[0; 0x24])))
        .with("story", Json::obj().with("x", 0u32).with("y", 0u32).with("cells", hex(&[0; STORY_ROWS * 18])))
        .with("course_plays", vec![Json::from(0u32); COURSES])
        .with("titles", hex(&[0; TITLES_LEN]))
        .with("backgrounds", hex(&[0; BACKGROUNDS_LEN]))
        .with("time_attack", Vec::<Json>::new());
    put_car(&mut doc, &car);
    doc
}

/// `saveProfile3`'s (0x0138) progress.
pub(super) fn save_profile3(doc: &mut Json, p: &[u8], now: u64) {
    doc.set("lv", r16(p, 0x26));
    doc.set("exp", r32(p, 0x28));
    doc.set("fame", r32(p, 0x4fc));
    doc.set("dpoint", r32(p, 0x4f8));
    doc.set("mileage", r32(p, 0x08));
    doc.set("access_time", now);
    let c = obj(doc, "chara");
    c.set("title", r16(p, 0x40));
    c.set("background", r8(p, 0x874));
    put_car(doc, &slice(p, 0x958, 0x9b8));
    let plays: Vec<Json> = (0..COURSES).map(|i| Json::from(r16(p, 0x554 + 2 * i))).collect();
    doc.set("course_plays", plays);
    let m = obj(doc, "missions");
    m.set("team", hex(&mission(&slice(p, 0x430, 0x452))));
    m.set("solo", hex(&mission(&slice(p, 0x848, 0x86a))));
    let mut cells = Vec::with_capacity(STORY_ROWS * 18);
    for i in 0..STORY_ROWS {
        let row = 0x1ac + i * 0x18;
        for j in 0..9 {
            cells.push(r8(p, row + j) as u8);
            cells.push(r8(p, row + 9 + j) as u8);
        }
    }
    doc.set("story", Json::obj().with("x", r16(p, 0x818)).with("y", r8(p, 0x7fc)).with("cells", hex(&cells)));
    merge_bits(doc, "titles", &slice(p, 0x42, 0xf6));
    doc.set(
        "unlocks",
        Json::obj().with("cup", r8(p, 0x110)).with("gauges", r16(p, 0x114)).with("music", r16(p, 0x140)).with("last_mileage_reward", r32(p, 0x13c)),
    );
    doc.set("settings", Json::obj().with("music", r16(p, 0x4ee)).with("pack", r32(p, 0x34)).with("paper_cup", r8(p, 0xf6)).with("gauges", r8(p, 0xf7)));
    // a free car ticket from its start, a free continue with both dates
    let (car_from, cont_from, cont_to) = (r32(p, 0x138), r32(p, 0x38), r32(p, 0x3c));
    let mut t = Json::obj();
    if car_from != 0 {
        t.set("free_car_from", car_from);
    }
    if cont_from != 0 && cont_to != 0 {
        t.set("free_continue_from", cont_from);
        t.set("free_continue_to", cont_to);
    }
    doc.set("tickets", t);
}

/// The player's team: (team id, leader).
pub(super) type TeamOf = Option<(u32, bool)>;

/// `loadProfile3`'s answer (0x012e, 0x0ea0 bytes).
pub(super) fn load_profile3(doc: &Json, team: TeamOf) -> Vec<u8> {
    let mut out = Out::new(0x0ea0, 0x012e);
    out.fill(0x07e4, 0x080c, 0xff);
    for ta in doc.a("time_attack") {
        let r = ta.u("route") as usize;
        if r >= ROUTES as usize {
            continue;
        }
        out.u32(0x00e4 + 4 * r, ta.u("timestamp") as u32);
        out.u16(0x067c + 2 * r, 0);
        // national rank
        out.u16(0x0184 + 2 * r, 0xffff);
        out.u32(0x05dc + 4 * r, ta.u("total_ms") as u32);
        out.u8(0x06cc + r, ta.u("flags") as u32);
        out.u8(0x07e4 + r, ta.u("grade") as u32);
        for (i, s) in ta.a("sections").iter().take(3).enumerate() {
            out.u16(0x06f4 + 6 * r + 2 * i, (s.as_u64().unwrap_or(0) >> 2) as u32);
        }
    }
    let story = doc.get("story").cloned().unwrap_or(Json::obj());
    let cells = bytes(&story, "cells", STORY_ROWS * 18);
    for i in 0..STORY_ROWS {
        out.put(0x0256 + i * 0x13, &cells[i * 18..i * 18 + 18]);
    }
    for (c, n) in doc.a("course_plays").iter().take(COURSES).enumerate() {
        out.u16(0x053c + 2 * c, n.as_u64().unwrap_or(0) as u32);
    }
    let t = doc.get("tickets").cloned().unwrap_or(Json::obj());
    if t.get("free_car_from").is_some() {
        out.u32(0x0214, t.u("free_car_from") as u32);
    }
    if t.get("free_continue_from").is_some() {
        out.u32(0x04b8, t.u("free_continue_from") as u32);
        out.u32(0x04bc, t.u("free_continue_to") as u32);
    }
    let u = doc.get("unlocks").cloned().unwrap_or(Json::obj());
    out.u8(0x00b4, u.u("cup") as u32);
    out.u16(0x00b8, u.u("gauges") as u32);
    out.u32(0x0218, u.u("last_mileage_reward") as u32);
    out.u16(0x021c, u.u("music") as u32);
    out.u16(0x0456, u32::from(team.is_some_and(|t| t.1)));
    let m = doc.get("missions").cloned().unwrap_or(Json::obj());
    out.put(0x0460, &bytes(&m, "team", 0x24));
    // minime: else a "model record" emblem on the profile card
    out.u16(0x0462, 0xffff);
    let s = doc.get("settings").cloned().unwrap_or(Json::obj());
    out.u32(0x0494, doc.u("aime_id") as u32);
    out.u32(0x0498, doc.u("mileage") as u32);
    out.u16(0x04a4, s.u("music") as u32);
    out.u16(0x04a8, doc.u("lv") as u32);
    out.u32(0x04ac, doc.u("exp") as u32);
    out.u32(0x04b4, s.u("pack") as u32);
    out.u32(0x04c4, doc.u("dpoint") as u32);
    out.u32(0x04e0, doc.u("fame") as u32);
    out.text(0x04ca, &unhex(doc.s("name"), doc.s("name").len() / 2), 0x15);
    out.u8(0x080c, story.u("y") as u32);
    out.u16(0x0828, story.u("x") as u32);
    out.put(0x0858, &bytes(&m, "solo", 0x24));
    out.put(0x0880, &chara_bytes(doc));
    out.put(0x0894, &bytes(doc, "titles", TITLES_LEN));
    out.u8(0x094d, s.u("paper_cup") as u32);
    out.u8(0x094e, s.u("gauges") as u32);
    out.u32(0x0954, team.map_or(0xffff_ffff, |t| t.0));
    out.u32(0x0dd0, doc.a("cars").len() as u32);
    out.put(0x0dd4, &selected_car(doc));
    out.done()
}

/// `saveTimeAttack1/2` (0x00cd / 0x0136): the route's best kept (non time attack credits send
/// a zero time).
pub(super) fn save_time_attack(doc: &mut Json, p: &[u8], now: u64) {
    let total = r32(p, 0x18);
    if total == 0 {
        return;
    }
    let route = r8(p, 0x54) >> 1;
    let score = Json::obj()
        .with("route", route)
        .with("timestamp", now)
        .with("flags", r8(p, 0x5c))
        .with("total_ms", total)
        .with("sections", vec![Json::from(r32(p, 0x24)), Json::from(r32(p, 0x28)), Json::from(r32(p, 0x2c))])
        .with("grade", r8(p, 0x62))
        .with("car", r16(p, 0x0c));
    let list = arr(doc, "time_attack");
    match list.iter_mut().find(|t| t.u("route") == route as u64) {
        Some(old) if (total as u64) < old.u("total_ms") => *old = score,
        Some(_) => {}
        None => list.push(score),
    }
}

// ── teams ─────────────────────────────────────────────────────────────────────────────

/// The auto teams' names (minime: Speed Stars, Red Suns, Night Kids), Shift-JIS.
const AUTO_TEAMS: [&[u8]; 3] = [
    // スピードスターズ
    &[0x83, 0x58, 0x83, 0x73, 0x81, 0x5b, 0x83, 0x68, 0x83, 0x58, 0x83, 0x5e, 0x81, 0x5b, 0x83, 0x59],
    // レッドサンズ
    &[0x83, 0x8c, 0x83, 0x62, 0x83, 0x68, 0x83, 0x54, 0x83, 0x93, 0x83, 0x59],
    // ナイトキッズ
    &[0x83, 0x69, 0x83, 0x43, 0x83, 0x67, 0x83, 0x4c, 0x83, 0x62, 0x83, 0x59],
];

/// The auto teams' names shown in English (`WAL_ALLS_IDZ_ENGLISH=1`, `idzenglish.rs`), in
/// full width Shift-JIS (the font of the game's names has no half width letters).
const AUTO_TEAMS_EN: [&str; 3] = ["Speed Stars", "Red Suns", "Night Kids"];

/// `text`'s letters, digits and spaces in full width Shift-JIS.
fn full_width_sjis(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in text.bytes() {
        match c {
            b'A'..=b'Z' => out.extend_from_slice(&[0x82, 0x60 + (c - b'A')]),
            b'a'..=b'z' => out.extend_from_slice(&[0x82, 0x81 + (c - b'a')]),
            b'0'..=b'9' => out.extend_from_slice(&[0x82, 0x4f + (c - b'0')]),
            _ => out.extend_from_slice(&[0x81, 0x40]),
        }
    }
    out
}

/// A team's name as shown: an auto team's (its Japanese name and full width serial) in English
/// with the English texts, else as stored.
fn team_name(team: &Json) -> Vec<u8> {
    let name = bytes(team, "name", team.s("name").len() / 2);
    if std::env::var("WAL_ALLS_IDZ_ENGLISH").is_ok_and(|v| v.trim() == "1") {
        for (ja, en) in AUTO_TEAMS.iter().zip(AUTO_TEAMS_EN) {
            if let Some(digits) = name.strip_prefix(*ja)
                && digits.len() == 6
                && digits.chunks(2).all(|c| c[0] == 0x82 && (0x4f..=0x58).contains(&c[1]))
            {
                let mut out = full_width_sjis(en);
                out.extend_from_slice(digits);
                return out;
            }
        }
    }
    name
}

const TEAM_SIZE: usize = 6;

/// The team of `aime`: (its index in `teams`, member index).
fn team_of(teams: &Json, aime: u32) -> Option<(usize, usize)> {
    teams.a("teams").iter().enumerate().find_map(|(t, team)| team.a("members").iter().position(|m| m.u("aime") == aime as u64).map(|m| (t, m)))
}

fn team_index(teams: &Json, id: u32) -> Option<usize> {
    teams.a("teams").iter().position(|t| t.u("id") == id as u64)
}

/// The player's team id and leadership, for its profile.
pub(super) fn find_team(teams: &Json, aime: u32) -> TeamOf {
    let (t, m) = team_of(teams, aime)?;
    let team = &teams.a("teams")[t];
    Some((team.u("id") as u32, team.a("members")[m].get("leader").and_then(Json::as_bool).unwrap_or(false)))
}

fn new_team(teams: &mut Json, name: &[u8], bg: u32, now: u64) -> u32 {
    let id = teams.u("next_id").max(1) as u32;
    teams.set("next_id", id + 1);
    let team = Json::obj().with("id", id).with("name", hex(name)).with("name_bg", bg).with("name_fx", 0u32).with("register_time", now).with("members", Vec::<Json>::new());
    arr(teams, "teams").push(team);
    id
}

/// `aime` in team `id` (out of its previous one): false when the team is full or gone.
fn join(teams: &mut Json, id: u32, aime: u32, leader: bool, now: u64) -> bool {
    let Some(t) = team_index(teams, id) else { return false };
    if teams.a("teams")[t].a("members").len() >= TEAM_SIZE && team_of(teams, aime).map(|x| x.0) != Some(t) {
        log!("idz: team {id} is full");
        return false;
    }
    leave(teams, aime);
    let team = &mut arr(teams, "teams")[t];
    arr(team, "members").push(Json::obj().with("aime", aime).with("leader", leader).with("join", now));
    true
}

/// `aime` out of its team.
fn leave(teams: &mut Json, aime: u32) {
    if let Some((t, m)) = team_of(teams, aime) {
        arr(&mut arr(teams, "teams")[t], "members").remove(m);
    }
}

fn make_leader(teams: &mut Json, id: u32, aime: u32) {
    if let Some(t) = team_index(teams, id) {
        for m in arr(&mut arr(teams, "teams")[t], "members") {
            let is = m.u("aime") == aime as u64;
            m.set("leader", is);
        }
    }
}

/// minime's `_fixupPrevTeam`: an empty team removed, a team without leader given its last
/// joined member.
fn fixup(teams: &mut Json, id: Option<u32>) {
    let Some(t) = id.and_then(|id| team_index(teams, id)) else { return };
    let list = arr(teams, "teams");
    let members = arr(&mut list[t], "members");
    if members.is_empty() {
        list.remove(t);
    } else if !members.iter().any(|m| m.get("leader").and_then(Json::as_bool).unwrap_or(false))
        && let Some(last) = members.iter_mut().max_by_key(|m| m.u("join"))
    {
        last.set("leader", true);
    }
}

/// minime's auto team: the last one while it has room (members and reservations), else the
/// next name / serial number; the player reserved in it until its profile is created.
pub(super) fn create_auto_team(teams: &mut Json, aime: u32, now: u64) -> u32 {
    let auto = teams.get("auto").cloned();
    let (serial, idx) = match &auto {
        Some(a) => {
            let id = a.u("team") as u32;
            if let Some(t) = team_index(teams, id) {
                let reserved = teams.a("reservations").iter().filter(|r| r.u("team") == id as u64 && r.u("aime") != aime as u64).count();
                if teams.a("teams")[t].a("members").len() + reserved < TEAM_SIZE {
                    reserve(teams, id, aime, false, now);
                    return id;
                }
            }
            // full (or gone): the next auto team
            let (serial, idx) = (a.u("serial") as u32, a.u("idx") as usize);
            if idx + 1 < AUTO_TEAMS.len() { (serial, idx + 1) } else { (serial + 1, 0) }
        }
        None => (1, 0),
    };
    // a three digit serial number in full width digits (Shift-JIS 0x824f + n)
    let mut name = AUTO_TEAMS[idx].to_vec();
    for d in [serial / 100 % 10, serial / 10 % 10, serial % 10] {
        name.extend_from_slice(&[0x82, 0x4f + d as u8]);
    }
    let id = new_team(teams, &name, idx as u32, now);
    teams.set("auto", Json::obj().with("serial", serial).with("idx", idx).with("team", id));
    reserve(teams, id, aime, true, now);
    id
}

fn reserve(teams: &mut Json, id: u32, aime: u32, leader: bool, now: u64) {
    let list = arr(teams, "reservations");
    list.retain(|r| r.u("aime") != aime as u64);
    list.push(Json::obj().with("aime", aime).with("team", id).with("leader", leader).with("join", now));
}

/// The reservation of `aime` made a membership (its profile created).
pub(super) fn commit_reservation(teams: &mut Json, aime: u32) {
    let list = arr(teams, "reservations");
    let Some(i) = list.iter().position(|r| r.u("aime") == aime as u64) else { return };
    let r = list.remove(i);
    let prev = find_team(teams, aime).map(|t| t.0);
    if join(teams, r.u("team") as u32, aime, r.get("leader").and_then(Json::as_bool).unwrap_or(false), r.u("join")) {
        fixup(teams, prev);
    }
}

/// minime's team answer (`loadTeam` 0x0078 / `createAutoTeam` 0x007c, 0x0ca0 bytes): the team,
/// its members (`players`: their documents).
fn team_answer(code: u16, team: Option<&Json>, players: &[Json]) -> Vec<u8> {
    team_answer_at(code, team, players, 0x011c)
}

/// The team answer with its members from `members_at` (Ver.2: 0x0120).
fn team_answer_at(code: u16, team: Option<&Json>, players: &[Json], members_at: usize) -> Vec<u8> {
    let mut out = Out::new(0x0ca0, code);
    let empty = Json::obj();
    let team = team.unwrap_or(&empty);
    out.u32(0x000c, team.u("id") as u32);
    out.text(0x0024, &team_name(team), 0x5c);
    out.u32(0x00d8, team.u("name_bg") as u32);
    out.u32(0x00dc, team.u("name_fx") as u32);
    // the unlocked backgrounds and effects, probably
    out.fill(0x00e0, 0x0101, 0xff);
    let leader = team.a("members").iter().find(|m| m.get("leader").and_then(Json::as_bool).unwrap_or(false)).map_or(0, |m| m.u("aime") as u32);
    out.u32(0x0080, leader);
    let mut i = 0;
    for m in team.a("members") {
        let Some(doc) = players.iter().find(|d| d.u("aime_id") == m.u("aime")) else { continue };
        if i == TEAM_SIZE {
            break;
        }
        let base = members_at + i * 0x5c;
        out.u32(base, doc.u("aime_id") as u32);
        out.text(base + 0x04, &bytes(doc, "name", doc.s("name").len() / 2), 0x13);
        out.u32(base + 0x18, doc.u("lv") as u32);
        // the month's points
        out.u32(base + 0x24, 0);
        out.u32(base + 0x34, doc.u("access_time") as u32);
        out.put(base + 0x44, &chara_bytes(doc));
        i += 1;
    }
    out.done()
}

// ── the other answers ─────────────────────────────────────────────────────────────────

/// The router's address, dotted.
fn host() -> String {
    let r = crate::netenv::router();
    format!("{}.{}.{}.{}", r[0], r[1], r[2], r[3])
}

/// `loadServerList` (0x0007): the servers on the router (minime's switchboard ports), the
/// news on the router's port 80 (ARTEMiS's routes).
pub(super) fn server_list(host: &str) -> Vec<u8> {
    server_list_at(host, 0)
}

/// The server list with its fields `o` bytes further (Ver.2: 2).
fn server_list_at(host: &str, o: usize) -> Vec<u8> {
    let mut out = Out::new(0x04b0, 0x0007);
    let h = host.as_bytes();
    out.u16(0x0002 + o, 1);
    out.text(0x0004 + o, h, 0x80);
    // userdb TCP / HTTP
    out.u16(0x0084 + o, 10000);
    out.u16(0x0086 + o, 10001);
    out.text(0x0088 + o, h, 0x80);
    // matching TCP, UDP send, UDP receive; tag matching
    for (i, port) in [10002u32, 10003, 10004, 10005, 10006, 10007].iter().enumerate() {
        out.u16(0x0108 + o + 2 * i, *port);
    }
    out.text(0x0114 + o, h, 0x80);
    out.u16(0x0194 + o, 10008);
    out.text(0x0198 + o, h, 0x80);
    out.u16(0x0218 + o, 10009);
    // ping return, echo 1, echo 2
    out.text(0x021c + o, h, 0x80);
    out.text(0x029c + o, h, 0x80);
    out.text(0x031c + o, h, 0x80);
    out.u16(0x039c + o, 10010);
    out.u16(0x039e + o, 10011);
    out.text(0x03a0 + o, format!("http://{host}/idz/news/news80**.txt").as_bytes(), 0x83);
    out.text(0x0424 + o, format!("http://{host}/idz/error").as_bytes(), 0x8b);
    out.done()
}

/// `loadTopTen1/2` (0x00b5 / 0x012c): up to 3 of the asked routes with results newer than
/// asked, their 10 best (every local player's best; minime v016: all of them, not only the
/// newer ones, else the newer time takes the first row), answered 0x00b6.
fn top_ten(p: &[u8], count: usize, times_at: usize, players: &[Json], teams: &Json) -> Vec<u8> {
    top_ten_as(p, count, 0x04, times_at, players, teams, false)
}

/// The top ten, the routes asked from `routes_at`; `v2`: Ver.2's answer (`loadTopTen3`,
/// 0x00ce: its fields 2 bytes further).
fn top_ten_as(p: &[u8], count: usize, routes_at: usize, times_at: usize, players: &[Json], teams: &Json, v2: bool) -> Vec<u8> {
    let (code, o) = if v2 { (0x00ce, 2) } else { (0x00b6, 0) };
    let mut out = Out::new(0x1720, code);
    let mut courses = 0;
    let mut seen = Vec::new();
    for i in 0..count {
        if courses == 3 {
            break;
        }
        let route = r16(p, routes_at + 2 * i) >> 1;
        let min = r32(p, times_at + 4 * i) as u64 + 1;
        if seen.contains(&route) {
            continue;
        }
        seen.push(route);
        let mut rows: Vec<(&Json, &Json)> =
            players.iter().flat_map(|d| d.a("time_attack").iter().filter(|t| t.u("route") == route as u64).map(move |t| (d, t))).collect();
        if !rows.iter().any(|(_, t)| t.u("timestamp") > min) {
            continue;
        }
        rows.sort_by_key(|(_, t)| (t.u("total_ms"), t.u("timestamp")));
        let outer = 0x0004 + o + 0x0794 * courses;
        out.u16(outer + o, route << 1);
        for (j, s) in rows[0].1.a("sections").iter().take(3).enumerate() {
            out.u32(outer + 0x04 + 4 * j, s.as_u64().unwrap_or(0) as u32);
        }
        for (j, (doc, ta)) in rows.iter().take(10).enumerate() {
            let inner = outer + 0x0010 + o + 0xc0 * j;
            out.u32(inner + 0x04, ta.u("total_ms") as u32);
            out.u32(inner + 0x08, ta.u("timestamp") as u32);
            out.u16(inner + 0x0c, ta.u("car") as u32);
            out.text(inner + 0x14, &bytes(doc, "name", doc.s("name").len() / 2), 0x13);
            let team = find_team(teams, doc.u("aime_id") as u32).and_then(|(id, _)| team_index(teams, id)).map(|t| &teams.a("teams")[t]);
            if let Some(team) = team {
                out.text(inner + 0x28, &team_name(team), 0x1f);
            }
            out.text(inner + 0x48, STORE_NAME, 0x2b);
        }
        courses += 1;
    }
    out.u16(0x0002 + o, courses as u32);
    // no trailers
    for i in 0..3 {
        out.u8(0x16c0 + 0x1c * i + 0x02, 0xff);
    }
    out.done()
}

/// `loadGhost` (0x00a1): minime's two empty ghosts.
fn ghost() -> Vec<u8> {
    ghost_as(0x00a1)
}

/// The ghosts answered `code` (Ver.2: 0x0096).
fn ghost_as(code: u16) -> Vec<u8> {
    let mut out = Out::new(0x0070, code);
    out.u16(0x0002, 0x0005);
    for i in 0..2 {
        let base = 0x04 + i * 0x34;
        out.u32(base + 0x04, 0xffff_ffff);
        for j in 0..3 {
            out.u32(base + 0x08 + 4 * j, 120000);
        }
        out.u16(base + 0x18, 0xffff);
    }
    out.done()
}

/// `loadGarage` (0x0091): the player's cars from the asked offset, 10 at most.
fn garage(doc: Option<&Json>, offset: usize) -> Vec<u8> {
    let mut out = Out::new(0x03d0, 0x0091);
    let cars: Vec<Vec<u8>> = doc.map_or(Vec::new(), |d| d.a("cars").iter().skip(offset).take(10).map(|c| bytes(c, "data", CAR_LEN)).collect());
    out.u16(0x0002, cars.len() as u32);
    for (i, car) in cars.iter().enumerate() {
        out.put(0x0004 + CAR_LEN * i, car);
    }
    out.done()
}

fn now() -> u64 {
    crate::clock::unix_now()
}

/// One request's answer (`p` its payload, code first).
pub(super) fn handle(proto: Proto, code: u16, p: &[u8]) -> Vec<u8> {
    if proto.v2() {
        return v2::handle(proto, code, p);
    }
    let _lock = store::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let store = Store(false);
    let aime = r32(p, 4);
    // the request's player, changed and saved
    let with_player = |f: &mut dyn FnMut(&mut Json)| match store.load(aime) {
        Some(mut doc) => {
            f(&mut doc);
            store.save(aime, &doc);
        }
        None => log!("idz: {} for {aime}: no profile", name(proto, code)),
    };
    let with_teams = |f: &mut dyn FnMut(&mut Json)| {
        let mut teams = store.load_teams();
        f(&mut teams);
        store.save_teams(&teams);
    };
    match code {
        0x0004 => {
            let mut out = Out::new(0x01a0, 0x0005);
            out.u8(0x0002, 1);
            // the server version
            out.u16(0x0016, 130);
            out.done()
        }
        0x00ab => {
            super::mark_ready();
            let mut out = Out::new(0x0230, 0x00ac);
            out.u8(0x0002, 1);
            out.done()
        }
        0x0006 => server_list(&host()),
        0x006b => {
            let mut out = Out::new(0x0010, 0x006c);
            out.u8(0x0004, u32::from(store.load(aime).is_some()));
            out.done()
        }
        0x0069 => {
            // unlocked (1), for an hour
            let mut out = Out::new(0x0020, 0x006a);
            out.u8(0x0018, 1);
            out.u16(0x001a, 0xffff);
            out.u32(0x001c, (now() + 3600) as u32);
            out.done()
        }
        0x006d => {
            let mut out = Out::new(0x0010, 0x006e);
            out.u8(0x0004, 1);
            out.done()
        }
        0x006f => {
            let mut out = Out::new(0x0010, 0x0070);
            out.u8(0x0004, 1);
            out.done()
        }
        0x0066 => {
            let doc = new_profile(p, now());
            log!("idz: new profile {aime} \"{}\"", printable(&cstr(p, 0x1e, 0x34)));
            store.save(aime, &doc);
            with_teams(&mut |t| commit_reservation(t, aime));
            // minime: the Aime id as the generic answer's status; ARTEMiS's at 0x08 too
            let mut out = generic(aime);
            out[8..12].copy_from_slice(&aime.to_le_bytes());
            out
        }
        0x012f => match store.load(aime) {
            Some(doc) => load_profile3(&doc, find_team(&store.load_teams(), aime)),
            None => {
                log!("idz: loadProfile3 for {aime}: no profile");
                generic(0)
            }
        },
        0x0138 => {
            with_player(&mut |d| save_profile3(d, p, now()));
            generic(1)
        }
        0x0067 | 0x0068 => {
            log!("idz: {} (v1.10's profile) not supported", name(proto, code));
            generic(0)
        }
        0x00cd | 0x0136 => {
            with_player(&mut |d| save_time_attack(d, p, now()));
            empty(0x00b0, 0x00ce)
        }
        0x0079 => {
            with_player(&mut |d| put_car(d, &slice(p, 0x20, 0x80)));
            empty(0x0010, 0x007a)
        }
        0x00a5 => {
            with_player(&mut |d| {
                d.set("settings", Json::obj().with("music", r16(p, 0x02)).with("pack", r32(p, 0x0c)).with("paper_cup", r8(p, 0x11)).with("gauges", r8(p, 0x12)))
            });
            generic(0)
        }
        0x00a6 => {
            with_player(&mut |d| {
                merge_bits(d, "backgrounds", &slice(p, 0x08, 0x2c));
                d.set("chara", chara(&slice(p, 0x9e, 0xb2)));
                let sel = r16(p, 0x9c);
                if d.a("cars").iter().any(|c| c.u("selector") == sel as u64) {
                    d.set("car", sel);
                } else {
                    log!("idz: saveStocker: no car {sel:#x}");
                }
            });
            generic(0)
        }
        0x00a7 => {
            let mut out = Out::new(0x00a0, 0x00a8);
            out.u8(0x0002, 1);
            if let Some(doc) = store.load(aime) {
                out.put(0x0003, &bytes(&doc, "backgrounds", BACKGROUNDS_LEN));
            }
            out.done()
        }
        0x0090 => garage(store.load(aime).as_ref(), r8(p, 0x08) as usize),
        // the car saved (minime v016's fix: v005 dropped it); status 0 is success here
        0x008e => {
            with_player(&mut |d| put_car(d, &slice(p, 0x08, 0x68)));
            empty(0x0010, 0x008f)
        }
        0x00b5 => top_ten(p, 32, 0x44, &store.players(), &store.load_teams()),
        0x012c => top_ten(p, 40, 0x54, &store.players(), &store.load_teams()),
        0x007b => {
            let mut teams = store.load_teams();
            let id = create_auto_team(&mut teams, aime, now());
            store.save_teams(&teams);
            let team = team_index(&teams, id).map(|t| &teams.a("teams")[t]);
            team_answer(0x007c, team, &store.players())
        }
        0x0077 => {
            let id = r32(p, 0x08);
            let teams = store.load_teams();
            let team = if id == 0xffff_ffff { None } else { team_index(&teams, id).map(|t| &teams.a("teams")[t]) };
            team_answer(0x0078, team, &store.players())
        }
        0x0071 => {
            let mut id = 0;
            with_teams(&mut |t| {
                let prev = find_team(t, aime).map(|x| x.0);
                id = new_team(t, &cstr(p, 0x08, 0x28), r8(p, 0x30), now());
                join(t, id, aime, true, now());
                make_leader(t, id, aime);
                fixup(t, prev);
            });
            let mut out = Out::new(0x0010, 0x0072);
            out.u32(0x0008, id);
            out.done()
        }
        0x0073 => {
            let (add, member, id) = (r8(p, 0x04) == 0, r32(p, 0x08), r32(p, 0x0c));
            with_teams(&mut |t| {
                if add {
                    let prev = find_team(t, member).map(|x| x.0);
                    if join(t, id, member, false, now()) && prev != Some(id) {
                        fixup(t, prev);
                    }
                } else {
                    leave(t, member);
                    fixup(t, Some(id));
                }
            });
            empty(0x0010, 0x0074)
        }
        0x008a => {
            with_teams(&mut |t| make_leader(t, r32(p, 0x08), aime));
            empty(0x0010, 0x008b)
        }
        0x0089 => {
            with_teams(&mut |t| {
                if let Some(i) = team_index(t, r32(p, 0x04)) {
                    let team = &mut arr(t, "teams")[i];
                    team.set("name_bg", r32(p, 0x08));
                    team.set("name_fx", r32(p, 0x0c));
                }
            });
            generic(0)
        }
        0x00a2 => empty(0x0010, 0x00a3),
        0x00b0 => empty(0x04c0, 0x00b1),
        0x0132 => empty(0x04c0, 0x0133),
        0x00b9 | 0x00bb => empty(0x0ba0, 0x00ba),
        0x0086 => empty(0x01c0, 0x0087),
        0x00a0 => ghost(),
        0x00be => empty(0x01b0, 0x00bf),
        0x00c1 => {
            // awarded today: no gacha
            let mut out = Out::new(0x0090, 0x00c2);
            out.u8(0x0002, 1);
            out.done()
        }
        0x009a => empty(0x05d0, 0x009b),
        0x0082 => empty(0x02b0, 0x0083),
        0x007f => empty(0x0220, 0x0080),
        0x013d => empty(0x04f0, 0x013e),
        // minime: 0x18ac bytes, padded to the cipher's blocks
        0x008c if r32(p, 0x04) != 0 => empty(0x17c0, 0x008d),
        0x013f if r32(p, 0x04) != 0 => empty(0x18b0, 0x0140),
        0x008c | 0x013f => generic(0),
        // loadGeneralReward, lockGarage, msg00AD, updateResult, team points, UI report, user log
        _ => generic(0),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A `createProfile` payload: Aime id, name, car (selector 0x0102), chara (title 7,
    /// background 9).
    pub(crate) fn create_request(aime: u32) -> Vec<u8> {
        let mut p = vec![0u8; 0xc0];
        p[0..2].copy_from_slice(&0x0066u16.to_le_bytes());
        p[4..8].copy_from_slice(&aime.to_le_bytes());
        p[0x1e..0x23].copy_from_slice(b"TAKUM");
        for i in 0..0x60 {
            p[0x40 + i] = i as u8;
        }
        p[0x40 + 0x44..0x40 + 0x46].copy_from_slice(&0x0102u16.to_le_bytes());
        p[0xa0] = 1;
        p[0xa2] = 0x22;
        p[0xb0] = 7;
        p[0xb2] = 9;
        p
    }

    #[test]
    fn create_then_load() {
        let doc = new_profile(&create_request(1234), 1_700_000_000);
        let out = load_profile3(&doc, Some((5, true)));
        assert_eq!(out.len(), 0x0ea0);
        assert_eq!(r16(&out, 0), 0x012e);
        assert_eq!(r32(&out, 0x0494), 1234);
        assert_eq!(&out[0x04ca..0x04d0], b"TAKUM\0");
        assert_eq!(r16(&out, 0x04a8), 1);
        assert_eq!(r32(&out, 0x04b4), 13640);
        assert_eq!(r8(&out, 0x094e), 5);
        assert_eq!(r16(&out, 0x00b8), 32);
        assert_eq!(r32(&out, 0x0954), 5);
        assert_eq!(r16(&out, 0x0456), 1);
        assert_eq!(r32(&out, 0x0dd0), 1);
        assert_eq!(r16(&out, 0x0dd4 + 0x44), 0x0102);
        assert_eq!(out[0x0dd4 + 0x10], 0x10);
        // chara: gender byte, background then title
        assert_eq!(out[0x0880], 1);
        assert_eq!(r16(&out, 0x0882), 0x22);
        assert_eq!(r16(&out, 0x0890), 9);
        assert_eq!(r16(&out, 0x0892), 7);
        assert_eq!(r16(&out, 0x0462), 0xffff);
        assert_eq!(out[0x07e4], 0xff);
    }

    #[test]
    fn save_then_load() {
        let mut doc = new_profile(&create_request(1), 1);
        let mut p = vec![0u8; 0x0a70];
        p[0..2].copy_from_slice(&0x0138u16.to_le_bytes());
        p[0x26] = 12;
        p[0x08..0x0c].copy_from_slice(&5000u32.to_le_bytes());
        p[0x4fc] = 3;
        p[0x42] = 0b101;
        p[0x874] = 4;
        p[0x40] = 6;
        p[0x554 + 2 * 3] = 9;
        // story row 1 cell 2: a 5, b 6
        p[0x1ac + 0x18 + 2] = 5;
        p[0x1ac + 0x18 + 9 + 2] = 6;
        p[0x818] = 2;
        p[0x7fc] = 1;
        // a new selected car's data
        p[0x958 + 0x44] = 0x02;
        p[0x958 + 0x45] = 0x01;
        p[0x958] = 0xaa;
        // team mission grid 0 slice 1: all bits (only 9 kept)
        p[0x430 + 0x0c] = 0xff;
        p[0x430 + 0x0d] = 0xff;
        save_profile3(&mut doc, &p, 2);
        let out = load_profile3(&doc, None);
        assert_eq!(r16(&out, 0x04a8), 12);
        assert_eq!(r32(&out, 0x0498), 5000);
        assert_eq!(r32(&out, 0x04e0), 3);
        assert_eq!(out[0x0894], 0b101);
        assert_eq!(r16(&out, 0x0890), 4);
        assert_eq!(r16(&out, 0x0892), 6);
        assert_eq!(r16(&out, 0x053c + 6), 9);
        assert_eq!(out[0x0256 + 0x13 + 4], 5);
        assert_eq!(out[0x0256 + 0x13 + 5], 6);
        assert_eq!(r16(&out, 0x0828), 2);
        assert_eq!(out[0x080c], 1);
        assert_eq!(out[0x0dd4], 0xaa);
        assert_eq!(r32(&out, 0x0dd0), 1);
        assert_eq!(r16(&out, 0x0460 + 0x0c), 0x1ff);
        assert_eq!(r32(&out, 0x0954), 0xffff_ffff);
        // the titles only gain bits
        p[0x42] = 0b010;
        save_profile3(&mut doc, &p, 3);
        assert_eq!(load_profile3(&doc, None)[0x0894], 0b111);
    }

    #[test]
    fn time_attack_best() {
        let mut doc = new_profile(&create_request(1), 1);
        let mut p = vec![0u8; 0x80];
        p[0x54] = 3 << 1;
        p[0x18..0x1c].copy_from_slice(&90_000u32.to_le_bytes());
        p[0x24..0x28].copy_from_slice(&30_000u32.to_le_bytes());
        p[0x62] = 2;
        save_time_attack(&mut doc, &p, 100);
        p[0x18..0x1c].copy_from_slice(&95_000u32.to_le_bytes());
        save_time_attack(&mut doc, &p, 101);
        let out = load_profile3(&doc, None);
        assert_eq!(r32(&out, 0x05dc + 12), 90_000);
        assert_eq!(r32(&out, 0x00e4 + 12), 100);
        assert_eq!(out[0x07e4 + 3], 2);
        assert_eq!(r16(&out, 0x06f4 + 18), 7500);
        // the top ten of route 3 since 0
        let mut q = vec![0u8; 0x110];
        q[4..6].copy_from_slice(&(3u16 << 1).to_le_bytes());
        let top = top_ten(&q, 40, 0x54, &[doc], &Json::obj());
        assert_eq!(r16(&top, 0), 0x00b6);
        assert_eq!(r16(&top, 2), 1);
        assert_eq!(r16(&top, 4), 6);
        assert_eq!(r32(&top, 4 + 0x10 + 4), 90_000);
        assert_eq!(&top[4 + 0x10 + 0x14..4 + 0x10 + 0x19], b"TAKUM");
    }

    #[test]
    fn auto_teams() {
        let mut teams = Json::obj().with("next_id", 1u32);
        let first = create_auto_team(&mut teams, 1, 10);
        assert_eq!(first, 1);
        commit_reservation(&mut teams, 1);
        assert_eq!(find_team(&teams, 1), Some((1, true)));
        for aime in 2..=6 {
            assert_eq!(create_auto_team(&mut teams, aime, 10), 1);
            commit_reservation(&mut teams, aime);
            assert_eq!(find_team(&teams, aime), Some((1, false)));
        }
        // full: Red Suns 001
        let next = create_auto_team(&mut teams, 7, 11);
        assert_eq!(next, 2);
        let team = &teams.a("teams")[1];
        assert_eq!(team.s("name"), "838c83628368835483938359824f824f8250");
        // SAFETY: tests of this module are the only readers of this variable
        unsafe { std::env::set_var("WAL_ALLS_IDZ_ENGLISH", "1") };
        assert_eq!(team_name(team), [&full_width_sjis("Red Suns")[..], &[0x82, 0x4f, 0x82, 0x4f, 0x82, 0x50]].concat());
        assert_eq!(team.u("name_bg"), 1);
        let docs: Vec<Json> = (1..=6).map(|a| new_profile(&create_request(a), 1)).collect();
        let out = team_answer(0x0078, Some(&teams.a("teams")[0]), &docs);
        assert_eq!(r32(&out, 0x0080), 1);
        assert_eq!(r32(&out, 0x011c + 5 * 0x5c), 6);
    }

    #[test]
    fn server_list_layout() {
        let out = server_list("192.168.1.1");
        assert_eq!(out.len(), 0x04b0);
        assert_eq!(&out[4..15], b"192.168.1.1");
        assert_eq!(r16(&out, 0x84), 10000);
        assert_eq!(r16(&out, 0x39e), 10011);
        assert!(out[0x3a0..].starts_with(b"http://192.168.1.1/idz/news/"));
    }
}
