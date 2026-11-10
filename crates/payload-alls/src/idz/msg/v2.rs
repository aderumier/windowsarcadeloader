//! Initial D ARCADE STAGE Zero Ver.2's messages (header protocol "210" / "230": v2.10 to v2.30),
//! ported from minime v016 (github.com/djhackersdev/minime, `src/idz/userdb`: the `funcList210`
//! readers, the `encode210` writers, the handlers) and ARTEMiS (`titles/idz/handlers`, version
//! indexes 2 and 3: v2.30's own codes, sizes). Most codes moved from Ver.1 (2.x's
//! `createProfile` is 0x0064, `lockProfile` 0x0065, `discoverProfile` 0x0067, ...); some
//! answers have their fields 2 or 4 bytes further, some requests their Aime id at 0x08.
//!
//! v2.30 (ARTEMiS): `loadProfile` 0x0142 (answered 0x0141, 0x1640 bytes; v2.10: 0x012f /
//! 0x012e, 0x1360), `saveProfile` 0x0143 (v2.10: 0x0138), `updateStoryClearNum` 0x0144
//! (answered 0x0145, 0x0800 bytes; v2.10: 0x013d / 0x013e, 0x0510), `updateUserLog` 0x00b3
//! (v2.10: 0x00ab). Both sets are answered whatever the header says. v2.30's layouts are
//! guessed as v2.10's (minime's `loadProfile4` / `saveProfile4` fields: the 2.30 answer's
//! extra bytes zero), the lengths of its own requests too (not sure: the framer takes what the
//! game sent, `idz.rs`). The players are kept apart from Ver.1's, in `Y:\idz2\`.

use wal_payload_common::json::Json;
use wal_payload_common::log;

use super::super::store::{self, Store, arr, bytes, hex, obj};
use super::*;

/// Ver.2's request payload lengths by code (minime's `msgLen`s, ARTEMiS's v2.30 codes), and
/// whether the length is sure.
pub(super) const REQUESTS: [(u16, usize, &str, bool); 48] = [
    (0x0004, 0x0050, "loadConfigA", true),
    (0x0006, 0x0020, "loadServerList", true),
    (0x0064, 0x00c0, "createProfile2", true),
    (0x0065, 0x0020, "lockProfile2", true),
    (0x0067, 0x0010, "discoverProfile2", true),
    (0x0069, 0x0020, "lockProfileExtend2", true),
    (0x006b, 0x0020, "unlockProfile2", true),
    (0x006d, 0x0050, "createTeam2", true),
    (0x006f, 0x0010, "updateTeamMember2", true),
    (0x0073, 0x0010, "loadTeam2", true),
    (0x0075, 0x0090, "saveNewCar2", true),
    (0x0077, 0x0010, "createAutoTeam2", true),
    (0x007b, 0x0010, "updateTeamPoints2", true),
    (0x007c, 0x0010, "updateProvisionalStoreRank2", true),
    (0x007e, 0x0410, "updateUiReport2", true),
    (0x007f, 0x0010, "loadRewardTable2", true),
    (0x0082, 0x0010, "saveTeamBanner2", true),
    (0x0083, 0x0020, "updateTeamLeader2", true),
    (0x0085, 0x0090, "saveGarage2", true),
    (0x0087, 0x0010, "loadGarage2", true),
    (0x0091, 0x0010, "saveTopic2", true),
    (0x0095, 0x0010, "loadGhost2", true),
    (0x0097, 0x0040, "checkTeamName2", true),
    (0x009a, 0x0020, "saveSettings2", true),
    (0x009b, 0x0110, "saveStocker2", true),
    (0x009c, 0x0010, "loadStocker2", true),
    (0x009e, 0x0010, "lockGarage2", true),
    (0x00a0, 0x0010, "loadConfigB2", true),
    (0x00a2, 0x0620, "unknownA2", true),
    (0x00a3, 0x0010, "load2on2RankingPoints2", true),
    (0x00a7, 0x0010, "loadTeamRanking2", true),
    (0x00a9, 0x0010, "loadTeamRanking4", true),
    (0x00ab, 0x0050, "updateUserLog2", true),
    (0x00ac, 0x0010, "loadEventInfo2", true),
    (0x00af, 0x0010, "loadGacha2", true),
    (0x00b3, 0x0050, "updateUserLog3", false),
    (0x00cc, 0x0030, "updateResult", true),
    (0x012c, 0x0110, "loadTopTen3", true),
    (0x012f, 0x0020, "loadProfile4", true),
    (0x0132, 0x0010, "load2on2Info", true),
    (0x0136, 0x0080, "saveTimeAttack3", true),
    (0x0138, 0x0eb0, "saveProfile4", true),
    (0x013b, 0x0010, "loadGeneralReward3", true),
    (0x013d, 0x0010, "updateStoryClearNum3", true),
    (0x013f, 0x0010, "saveExpedition2", true),
    (0x0142, 0x0020, "loadProfile5", false),
    (0x0143, 0x1190, "saveProfile5", true),
    (0x0144, 0x0010, "updateStoryClearNum4", false),
];

pub(super) fn request_info(proto: Proto, code: u16) -> Option<(usize, bool)> {
    let (_, len, _, sure) = REQUESTS.iter().find(|r| r.0 == code)?;
    // v2.30's 0x00ab: not v2.10's user log (ARTEMiS: 0x00b3), unknown
    Some((*len, *sure && !(proto == Proto::V230 && code == 0x00ab)))
}

pub(super) fn name(code: u16) -> &'static str {
    REQUESTS.iter().find(|r| r.0 == code).map_or("unknown", |r| r.2)
}

/// minime's `generic2` (0x0030 bytes): the status at 0x04 and 0x08 (createProfile's Aime id).
fn generic2(status: u32) -> Vec<u8> {
    let mut out = Out::new(0x30, 0x0001);
    out.u32(0x04, status);
    out.u32(0x08, status);
    out.done()
}

const TITLES2_LEN: usize = 0x177;
const BACKGROUNDS2_LEN: usize = 0x2c;
const MY_CHARA_LEN: usize = 0x64;
const STAMPS_LEN: usize = 0x26;
/// The story: 6 chapters of 28 cells (a, b, c: 4 bytes each).
const CHAPTERS: usize = 6;
const CELLS: usize = 28;
const STORY2_LEN: usize = CHAPTERS * CELLS * 4;

/// `createProfile2`'s new player (minime's defaults; the chara at 0xa8).
pub(super) fn new_profile2(p: &[u8], now: u64) -> Json {
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
        .with("chara", chara(&slice(p, 0xa8, 0xbc)))
        .with("car", car_selector(&car))
        .with(
            "settings",
            Json::obj().with("music", 0u32).with("pack", 13640u32).with("aura", 0u32).with("paper_cup", 0u32).with("gauges", 5u32).with("driving_style", 0u32),
        )
        .with("unlocks", Json::obj().with("auras", 1u32).with("cup", 0u32).with("gauges", 1u32 << 5).with("music", 0u32).with("last_mileage_reward", 0u32))
        .with("tickets", Json::obj())
        .with("missions", Json::obj().with("team", hex(&[0; 0x24])).with("solo", hex(&[0; 0x24])))
        .with("story", Json::obj().with("x", 0u32).with("y", 0u32).with("cells", hex(&[0; STORY2_LEN])))
        .with("course_plays", vec![Json::from(0u32); COURSES])
        .with("titles", hex(&[0; TITLES2_LEN]))
        .with("backgrounds", hex(&[0; BACKGROUNDS2_LEN]))
        .with("my_chara", hex(&[0; MY_CHARA_LEN]))
        .with("stamps", hex(&[0; STAMPS_LEN]))
        .with("selected_stamps", hex(&[0; 8]))
        .with("weekly", hex(&[0; 12]))
        .with("weekly_reset", 0u32)
        .with("time_attack", Vec::<Json>::new());
    put_car(&mut doc, &car);
    doc
}

/// `saveProfile4`'s (0x0138; v2.30's 0x0143 taken as the same) progress.
pub(super) fn save_profile4(doc: &mut Json, p: &[u8], now: u64) {
    doc.set("lv", r16(p, 0x26));
    doc.set("exp", r32(p, 0x28));
    doc.set("fame", r32(p, 0x784));
    doc.set("dpoint", r32(p, 0x780));
    doc.set("mileage", r32(p, 0x08));
    doc.set("access_time", now);
    let c = obj(doc, "chara");
    c.set("title", r16(p, 0x40));
    c.set("background", r8(p, 0xafc));
    put_car(doc, &slice(p, 0xb80, 0xbe0));
    let plays: Vec<Json> = (0..COURSES).map(|i| Json::from(r16(p, 0x7dc + 2 * i))).collect();
    doc.set("course_plays", plays);
    let m = obj(doc, "missions");
    m.set("team", hex(&mission(&slice(p, 0x6c0, 0x6e4))));
    m.set("solo", hex(&mission(&slice(p, 0xad0, 0xaf2))));
    // minime: the rows' first 2 bytes (the chapter) skipped, cells a, b, c
    let mut cells = vec![0u8; STORY2_LEN];
    for i in 0..CHAPTERS {
        let row = 0x26c + 0x02 + i * 0x7a;
        for j in 0..CELLS {
            for k in 0..3 {
                cells[(i * CELLS + j) * 4 + k] = r8(p, row + j * 4 + k) as u8;
            }
        }
    }
    doc.set("story", Json::obj().with("x", r16(p, 0xaa0)).with("y", r8(p, 0xa84)).with("cells", hex(&cells)));
    merge_bits(doc, "titles", &slice(p, 0x42, 0x1b9));
    merge_bits(doc, "stamps", &slice(p, 0xd28, 0xd4e));
    doc.set("selected_stamps", hex(&slice(p, 0xd74, 0xd7c)));
    doc.set("weekly", hex(&slice(p, 0xc84, 0xc90)));
    doc.set("weekly_reset", r32(p, 0xc90));
    doc.set(
        "unlocks",
        Json::obj()
            .with("auras", r16(p, 0x1d0))
            .with("cup", r8(p, 0x1d4))
            .with("gauges", r32(p, 0x1d8))
            .with("music", r16(p, 0x204))
            .with("last_mileage_reward", r32(p, 0x200)),
    );
    doc.set(
        "settings",
        Json::obj()
            .with("music", r16(p, 0x776))
            .with("pack", r32(p, 0x34))
            .with("aura", r8(p, 0x2c))
            .with("paper_cup", r8(p, 0x1b9))
            .with("gauges", r8(p, 0x1ba))
            .with("driving_style", r8(p, 0xdae)),
    );
    let (car_from, cont_from, cont_to) = (r32(p, 0x1fc), r32(p, 0x38), r32(p, 0x3c));
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

/// The chara as Ver.2 sends it (0x16 bytes: the title at 0x14).
fn chara2_bytes(doc: &Json) -> Vec<u8> {
    let c = doc.get("chara").cloned().unwrap_or(Json::obj());
    let mut out = bytes(&c, "head", 0x10);
    out.extend_from_slice(&(c.u("background") as u16).to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(c.u("title") as u16).to_le_bytes());
    out
}

/// v2.30's profile: v2.10's with 6 more story chapters after its 6 (rows of 0x7a bytes: 0x2dc).
/// The save request (0x1190 = 0xeb0 + 0x2e0): the rows from 0x548 (the 7th row's chapter number;
/// the rows' numbers 1-11 every 0x7a bytes from 0x26c), v2.10's fields after them 0x2d8 bytes
/// further (createProfile's car found there), 8 bytes more at the end. The load answer (0x1640 =
/// 0x1360 + 0x2e0): the rows at 0x534, v2.10's fields 0x2dc further (found by a probe answer:
/// the Aime id read at 0x9b8, v2.10's 0x6dc; the team id at 0xf3c, 0xc60), 4 bytes more at the
/// end; the car 4 bytes back (see `load_profile5`). The new rows are kept as they came
/// (`story2`, hex) and given back.
const STORY230_LEN: usize = 0x2dc;
const SAVE_SHIFT230: usize = 0x2d8;
const TAIL230_LEN: usize = 8;
const STORY230_LOAD_AT: usize = 0x0258 + 6 * 0x7a;
const STORY230_SAVE_AT: usize = 0x026c + 6 * 0x7a;

/// v2.30's load answer: v2.10's with the extra story chapters inserted.
fn load_profile5(doc: &Json, team: TeamOf) -> Vec<u8> {
    let mut out = load_profile4(doc, team, 0x0141, 0x1360);
    let mut extra = unhex(doc.s("story2"), STORY230_LEN);
    extra.resize(STORY230_LEN, 0);
    out.splice(STORY230_LOAD_AT..STORY230_LOAD_AT, extra);
    // the selected car right after the car count in 2.30 (0x1564, the car at 0x1568: the game's
    // answer parser at InitialD0_DX11_Nu.exe 0x1402d0b61 copies it from there; 2.10 left 4 bytes
    // between them): v2.10's car, 0x2dc further, moved 4 bytes back
    let car = 0x1290 + STORY230_LEN;
    out.copy_within(car..car + 0x60, car - 4);
    out[car + 0x5c..car + 0x60].fill(0);
    out.resize(0x1640, 0);
    if std::env::var("WAL_ALLS_IDZ_PROBE").is_ok_and(|v| v.trim() == "1") {
        // debug: each empty 4-byte slot holds its own offset (0x50000000 + offset): the values
        // the game sends back tell where it reads them
        for at in (0x0004..0x1640).step_by(4) {
            if out[at..at + 4] == [0; 4] {
                out[at..at + 4].copy_from_slice(&(0x5000_0000u32 + at as u32).to_le_bytes());
            }
        }
    }
    out
}

/// v2.30's save request: the extra story chapters kept, the rest read as v2.10's.
fn save_profile5(doc: &mut Json, p: &[u8], now: u64) {
    if p.len() < STORY230_SAVE_AT + STORY230_LEN {
        save_profile4(doc, p, now);
        return;
    }
    let extra = &p[STORY230_SAVE_AT..STORY230_SAVE_AT + STORY230_LEN];
    let mut v210: Vec<u8> = [&p[..STORY230_SAVE_AT], &p[STORY230_SAVE_AT + SAVE_SHIFT230..]].concat();
    v210.truncate(p.len() - SAVE_SHIFT230 - TAIL230_LEN);
    save_profile4(doc, &v210, now);
    doc.set("story2", hex(extra));
}

/// `loadProfile4`'s answer (0x012e, 0x1360 bytes).
pub(super) fn load_profile4(doc: &Json, team: TeamOf, code: u16, size: usize) -> Vec<u8> {
    let mut out = Out::new(size, code);
    let s = doc.get("settings").cloned().unwrap_or(Json::obj());
    let u = doc.get("unlocks").cloned().unwrap_or(Json::obj());
    let m = doc.get("missions").cloned().unwrap_or(Json::obj());
    out.u16(0x06f0, doc.u("lv") as u32);
    out.u32(0x06f4, doc.u("exp") as u32);
    out.u32(0x070c, doc.u("dpoint") as u32);
    out.u32(0x0728, doc.u("fame") as u32);
    out.u32(0x06dc, doc.u("aime_id") as u32);
    out.u32(0x0c60, team.map_or(0xffff_ffff, |t| t.0));
    out.u32(0x06e0, doc.u("mileage") as u32);
    out.put(0x0aa0, &bytes(&m, "solo", 0x24));
    out.put(0x0ac8, &chara2_bytes(doc));
    out.put(0x0ade, &bytes(doc, "titles", TITLES_LEN));
    out.u32(0x06fc, s.u("pack") as u32);
    out.u8(0x0c57, s.u("aura") as u32);
    out.u8(0x0c5a, s.u("paper_cup") as u32);
    out.u8(0x0c5b, s.u("gauges") as u32);
    out.u8(0x1184, s.u("driving_style") as u32);
    out.u16(0x06ec, s.u("music") as u32);
    out.text(0x0712, &bytes(doc, "name", doc.s("name").len() / 2), 0x15);
    out.put(0x1290, &selected_car(doc));
    out.u32(0x1288, doc.a("cars").len() as u32);
    out.u16(0x00b0, u.u("auras") as u32);
    out.u8(0x00b4, u.u("cup") as u32);
    out.u32(0x00b8, u.u("gauges") as u32);
    out.u32(0x0218, u.u("last_mileage_reward") as u32);
    out.u16(0x021c, u.u("music") as u32);
    out.u16(0x06a0, u32::from(team.is_some_and(|t| t.1)));
    out.u32(0x1068, doc.u("weekly_reset") as u32);
    out.put(0x105a, &bytes(doc, "weekly", 12));
    // minime: the stamps twice, as new and old ones
    let stamps = bytes(doc, "stamps", STAMPS_LEN);
    out.put(0x10f8, &stamps);
    out.put(0x111e, &stamps);
    out.put(0x1144, &bytes(doc, "selected_stamps", 8));
    let t = doc.get("tickets").cloned().unwrap_or(Json::obj());
    if t.get("free_car_from").is_some() {
        out.u32(0x0214, t.u("free_car_from") as u32);
    }
    if t.get("free_continue_from").is_some() {
        out.u32(0x0700, t.u("free_continue_from") as u32);
        out.u32(0x0704, t.u("free_continue_to") as u32);
    }
    for (c, n) in doc.a("course_plays").iter().take(COURSES).enumerate() {
        out.u16(0x0784 + 2 * c, n.as_u64().unwrap_or(0) as u32);
    }
    for ta in doc.a("time_attack") {
        let r = ta.u("route") as usize;
        if r >= ROUTES as usize {
            continue;
        }
        out.u32(0x00e4 + 4 * r, ta.u("timestamp") as u32);
        // national rank
        out.u16(0x0184 + 2 * r, 0xffff);
        out.u32(0x0824 + 4 * r, ta.u("total_ms") as u32);
        out.u8(0x0914 + r, ta.u("flags") as u32);
        out.u8(0x0a2c + r, ta.u("grade") as u32);
        for (i, sec) in ta.a("sections").iter().take(3).enumerate() {
            out.u16(0x093c + 6 * r + 2 * i, (sec.as_u64().unwrap_or(0) >> 2) as u32);
        }
    }
    let story = doc.get("story").cloned().unwrap_or(Json::obj());
    let cells = bytes(&story, "cells", STORY230_LEN);
    for i in 0..CHAPTERS {
        let row = 0x0258 + i * 0x7a;
        for j in 0..CELLS {
            let c = &cells[(i * CELLS + j) * 4..(i * CELLS + j) * 4 + 4];
            out.u8(row + j * 4, c[0] as u32);
            out.u8(row + j * 4 + 1, c[1] as u32);
            out.u16(row + j * 4 + 2, c[2] as u32);
        }
    }
    out.u8(0x0a54, story.u("y") as u32);
    out.u16(0x0a70, story.u("x") as u32);
    out.done()
}

/// `loadGarage2` (0x0088): the cars from the asked offset, 10 at most, 0x68 bytes apart.
fn garage2(doc: Option<&Json>, offset: usize) -> Vec<u8> {
    let mut out = Out::new(0x0420, 0x0088);
    let cars: Vec<Vec<u8>> = doc.map_or(Vec::new(), |d| d.a("cars").iter().skip(offset).take(10).map(|c| bytes(c, "data", CAR_LEN)).collect());
    out.u16(0x0004, cars.len() as u32);
    for (i, car) in cars.iter().enumerate() {
        out.put(0x0008 + 0x68 * i, car);
    }
    out.done()
}

/// The server version answered to `loadConfigA` (`WAL_ALLS_IDZ_SERVER_VERSION` else): the
/// client's own, 230 for 2.30 (as ARTEMiS answers; a 2.30 client answered 210, minime's, never
/// asks for loadConfigB, plays offline and refuses the Aime cards: error 6503), 210 for 2.10.
fn server_version(proto: Proto) -> u32 {
    let default = if proto == Proto::V230 { 230 } else { 210 };
    std::env::var("WAL_ALLS_IDZ_SERVER_VERSION").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// The Aime id of a Ver.2 request: at 0x08 for some (minime's decoders), else 0x04.
fn aime_of(code: u16, p: &[u8]) -> u32 {
    match code {
        0x009a | 0x0136 => r32(p, 0x08),
        _ => r32(p, 0x04),
    }
}

/// One Ver.2 request's answer.
pub(super) fn handle(proto: Proto, code: u16, p: &[u8]) -> Vec<u8> {
    let _lock = store::LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let store = Store(true);
    let aime = aime_of(code, p);
    let with_player = |f: &mut dyn FnMut(&mut Json)| match store.load(aime) {
        Some(mut doc) => {
            f(&mut doc);
            store.save(aime, &doc);
        }
        None => log!("idz: {} for {aime}: no profile", name(code)),
    };
    let with_teams = |f: &mut dyn FnMut(&mut Json)| {
        let mut teams = store.load_teams();
        f(&mut teams);
        store.save_teams(&teams);
    };
    match code {
        0x0004 => {
            let mut out = Out::new(0x05e0, 0x0005);
            out.u8(0x0002, 1);
            out.u16(0x0016, server_version(proto));
            out.done()
        }
        0x00a0 => {
            super::super::mark_ready();
            let mut out = Out::new(0x0240, 0x00a1);
            out.u8(0x0002, 1);
            out.done()
        }
        0x0006 => server_list_at(&host(), 2),
        0x0064 => {
            let doc = new_profile2(p, now());
            log!("idz: new Ver.2 profile {aime} \"{}\"", printable(&cstr(p, 0x1e, 0x34)));
            store.save(aime, &doc);
            with_teams(&mut |t| commit_reservation(t, aime));
            // minime: the Aime id at 0x08, ARTEMiS (v2.30) at 0x04
            generic2(aime)
        }
        0x0065 => {
            let mut out = Out::new(0x0020, 0x0066);
            out.u8(0x0018, 1);
            out.u16(0x001a, 0xffff);
            out.u32(0x001c, (now() + 3600) as u32);
            out.done()
        }
        0x0067 => {
            let mut out = Out::new(0x0010, 0x0068);
            out.u8(0x0004, u32::from(store.load(aime).is_some()));
            out.done()
        }
        0x0069 | 0x006b => {
            let mut out = Out::new(0x0010, code + 1);
            out.u8(0x0004, 1);
            out.done()
        }
        0x012f | 0x0142 => {
            match store.load(aime) {
                Some(doc) if code == 0x0142 => load_profile5(&doc, find_team(&store.load_teams(), aime)),
                Some(doc) => load_profile4(&doc, find_team(&store.load_teams(), aime), 0x012e, 0x1360),
                None => {
                    log!("idz: {} for {aime}: no profile", name(code));
                    generic2(0)
                }
            }
        }
        0x0138 => {
            with_player(&mut |d| save_profile4(d, p, now()));
            generic2(1)
        }
        0x0143 => {
            with_player(&mut |d| save_profile5(d, p, now()));
            generic2(1)
        }
        0x0136 => {
            // saveTimeAttack3: saveTimeAttack2's fields 4 bytes further
            with_player(&mut |d| save_time_attack(d, p.get(4..).unwrap_or(&[]), now()));
            empty(0x00f0, 0x00cd)
        }
        0x0075 => {
            with_player(&mut |d| put_car(d, &slice(p, 0x20, 0x80)));
            empty(0x0010, 0x0076)
        }
        0x0085 => {
            with_player(&mut |d| put_car(d, &slice(p, 0x08, 0x68)));
            empty(0x0010, 0x0086)
        }
        0x0087 => garage2(store.load(aime).as_ref(), r8(p, 0x08) as usize),
        0x009a => {
            with_player(&mut |d| {
                d.set(
                    "settings",
                    Json::obj()
                        .with("music", r16(p, 0x04))
                        .with("pack", r32(p, 0x10))
                        .with("paper_cup", r8(p, 0x15))
                        .with("gauges", r8(p, 0x16))
                        .with("aura", r8(p, 0x17))
                        .with("driving_style", r8(p, 0x18)),
                )
            });
            generic2(0)
        }
        0x009b => {
            with_player(&mut |d| {
                merge_bits(d, "backgrounds", &slice(p, 0x08, 0x34));
                merge_bits(d, "my_chara", &slice(p, 0x34, 0x98));
                merge_bits(d, "stamps", &slice(p, 0xb3, 0xd9));
                d.set("selected_stamps", hex(&slice(p, 0xda, 0xe2)));
                d.set("chara", chara(&slice(p, 0x9e, 0xb2)));
                let sel = r16(p, 0x9c);
                if d.a("cars").iter().any(|c| c.u("selector") == sel as u64) {
                    d.set("car", sel);
                } else {
                    log!("idz: saveStocker2: no car {sel:#x}");
                }
            });
            generic2(0)
        }
        0x009c => {
            let mut out = Out::new(0x00a0, 0x009d);
            out.u8(0x0004, 1);
            if let Some(doc) = store.load(aime) {
                out.put(0x0005, &bytes(&doc, "backgrounds", BACKGROUNDS2_LEN));
                out.put(0x0031, &bytes(&doc, "my_chara", MY_CHARA_LEN));
            }
            out.done()
        }
        0x012c => top_ten_as(p, 40, 0x06, 0x58, &store.players(), &store.load_teams(), true),
        0x0077 => {
            let mut teams = store.load_teams();
            let id = create_auto_team(&mut teams, aime, now());
            store.save_teams(&teams);
            let team = team_index(&teams, id).map(|t| &teams.a("teams")[t]);
            team_answer_at(0x0078, team, &store.players(), 0x0120)
        }
        0x0073 => {
            let id = r32(p, 0x08);
            let teams = store.load_teams();
            let team = if id == 0xffff_ffff { None } else { team_index(&teams, id).map(|t| &teams.a("teams")[t]) };
            team_answer_at(0x0074, team, &store.players(), 0x0120)
        }
        0x006d => {
            let mut id = 0;
            with_teams(&mut |t| {
                let prev = find_team(t, aime).map(|x| x.0);
                id = new_team(t, &cstr(p, 0x08, 0x28), r8(p, 0x30), now());
                join(t, id, aime, true, now());
                make_leader(t, id, aime);
                fixup(t, prev);
            });
            let mut out = Out::new(0x0010, 0x006e);
            out.u32(0x0008, id);
            out.done()
        }
        0x006f => {
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
            empty(0x0010, 0x0070)
        }
        0x0083 => {
            with_teams(&mut |t| make_leader(t, r32(p, 0x08), aime));
            empty(0x0010, 0x0084)
        }
        0x0082 => {
            with_teams(&mut |t| {
                if let Some(i) = team_index(t, r32(p, 0x04)) {
                    let team = &mut arr(t, "teams")[i];
                    team.set("name_bg", r32(p, 0x08));
                    team.set("name_fx", r32(p, 0x0c));
                }
            });
            generic2(0)
        }
        0x0097 => empty(0x0010, 0x0098),
        0x00a3 => empty(0x1290, 0x00a4),
        0x0132 => empty(0x0540, 0x0133),
        0x00a7 | 0x00a9 => empty(0x0ba0, 0x00a8),
        0x007f => empty(0x01c0, 0x0080),
        0x0095 => ghost_as(0x0096),
        0x00ac => empty(0x01b0, 0x00ad),
        0x00af => {
            // awarded today: no gacha
            let mut out = Out::new(0x0090, 0x00b0);
            out.u8(0x0004, 1);
            out.done()
        }
        0x0091 => empty(0x05d0, 0x0092),
        0x007c => empty(0x02b0, 0x007d),
        0x013d => empty(0x0510, 0x013e),
        0x0144 => empty(0x0800, 0x0145),
        // 2.30: the reward list answer (minime's 0x009d) is retried three times then the login
        // fails ("[accessStep] FAILED" in the game's log, the player plays without data): the
        // generic answer, as 1.30 is answered
        0x013b if proto == Proto::V230 => generic2(0),
        0x013b => empty(0x0330, 0x009d),
        0x013f if r32(p, 0x04) != 0 => empty(0x18b0, 0x0140),
        // v2.30's 0x00ab (unknown), lockGarage, unknownA, updateResult, team points, UI report,
        // user log, saveExpedition without data, the unknown codes: ARTEMiS's generic answer
        _ => {
            if proto == Proto::V230 && code == 0x00ab || request_info(proto, code).is_none() {
                return Out::new(0x30, 0x0001).done();
            }
            generic2(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `createProfile2` payload: Aime id, name, car (selector 0x0102), chara at 0xa8 (title
    /// 7, background 9).
    fn create_request2(aime: u32) -> Vec<u8> {
        let mut p = vec![0u8; 0xc0];
        p[0..2].copy_from_slice(&0x0064u16.to_le_bytes());
        p[4..8].copy_from_slice(&aime.to_le_bytes());
        p[0x1e..0x23].copy_from_slice(b"TAKUM");
        for i in 0..0x60 {
            p[0x40 + i] = i as u8;
        }
        p[0x40 + 0x44..0x40 + 0x46].copy_from_slice(&0x0102u16.to_le_bytes());
        p[0xa8] = 1;
        p[0xaa] = 0x22;
        p[0xb8] = 7;
        p[0xba] = 9;
        p
    }

    #[test]
    fn create_then_load_230() {
        let doc = new_profile2(&create_request2(4321), 1_700_000_000);
        let out = load_profile5(&doc, Some((5, true)));
        assert_eq!(out.len(), 0x1640);
        // as v2.10's: without the extra story chapters (the car then 4 bytes back)
        let out: Vec<u8> = [&out[..STORY230_LOAD_AT], &out[STORY230_LOAD_AT + STORY230_LEN..]].concat();
        assert_eq!(r16(&out, 0), 0x0141);
        assert_eq!(r32(&out, 0x06dc), 4321);
        assert_eq!(&out[0x0712..0x0718], b"TAKUM\0");
        assert_eq!(r16(&out, 0x06f0), 1);
        assert_eq!(r32(&out, 0x06fc), 13640);
        assert_eq!(out[0x0c5b], 5);
        assert_eq!(r32(&out, 0x00b8), 32);
        assert_eq!(r16(&out, 0x00b0), 1);
        assert_eq!(r32(&out, 0x0c60), 5);
        assert_eq!(r16(&out, 0x06a0), 1);
        assert_eq!(r32(&out, 0x1288), 1);
        assert_eq!(r16(&out, 0x1290 - 4 + 0x44), 0x0102);
        // chara2: gender, background at 0x10, title at 0x14
        assert_eq!(out[0x0ac8], 1);
        assert_eq!(r16(&out, 0x0aca), 0x22);
        assert_eq!(r16(&out, 0x0ac8 + 0x10), 9);
        assert_eq!(r16(&out, 0x0ac8 + 0x14), 7);
        // v2.10's answer: same fields, its own code and size
        let out = load_profile4(&doc, None, 0x012e, 0x1360);
        assert_eq!(out.len(), 0x1360);
        assert_eq!(r16(&out, 0), 0x012e);
        assert_eq!(r32(&out, 0x0c60), 0xffff_ffff);
    }

    #[test]
    fn save_then_load_230() {
        let mut doc = new_profile2(&create_request2(1), 1);
        let mut p = vec![0u8; 0x0eb0];
        p[0..2].copy_from_slice(&0x0143u16.to_le_bytes());
        p[0x26] = 12;
        p[0x08..0x0c].copy_from_slice(&5000u32.to_le_bytes());
        p[0x784] = 3;
        p[0x42] = 0b101;
        p[0x40] = 6;
        p[0xafc] = 4;
        p[0x7dc + 6] = 9;
        // chapter 1 cell 2: a 5, b 6, c 7
        let cell = 0x26e + 0x7a + 2 * 4;
        p[cell] = 5;
        p[cell + 1] = 6;
        p[cell + 2] = 7;
        p[0xaa0] = 2;
        p[0xa84] = 1;
        p[0xb80 + 0x44] = 0x02;
        p[0xb80 + 0x45] = 0x01;
        p[0xb80] = 0xaa;
        p[0xdae] = 1;
        p[0xd28] = 0x81;
        // v2.30's request: the extra story chapters after the 6 of v2.10
        let p: Vec<u8> = [&p[..STORY230_SAVE_AT], &[7u8; SAVE_SHIFT230][..], &p[STORY230_SAVE_AT..], &[0u8; TAIL230_LEN][..]].concat();
        assert_eq!(p.len(), 0x1190);
        save_profile5(&mut doc, &p, 2);
        let out = load_profile5(&doc, None);
        assert_eq!((r16(&out, 0), out.len()), (0x0141, 0x1640));
        assert!(out[STORY230_LOAD_AT..STORY230_LOAD_AT + SAVE_SHIFT230].iter().all(|&b| b == 7));
        // the Aime id where 2.30 reads it (a probe answer)
        assert_eq!(r32(&out, 0x09b8), 1);
        // the rest as v2.10's
        let out: Vec<u8> = [&out[..STORY230_LOAD_AT], &out[STORY230_LOAD_AT + STORY230_LEN..]].concat();
        assert_eq!(r16(&out, 0x06f0), 12);
        assert_eq!(r32(&out, 0x06e0), 5000);
        assert_eq!(r32(&out, 0x0728), 3);
        assert_eq!(out[0x0ade], 0b101);
        assert_eq!(r16(&out, 0x0ac8 + 0x10), 4);
        assert_eq!(r16(&out, 0x0ac8 + 0x14), 6);
        assert_eq!(r16(&out, 0x0784 + 6), 9);
        let row = 0x0258 + 0x7a + 2 * 4;
        assert_eq!((out[row], out[row + 1], r16(&out, row + 2)), (5, 6, 7));
        assert_eq!(r16(&out, 0x0a70), 2);
        assert_eq!(out[0x0a54], 1);
        assert_eq!(out[0x1290 - 4], 0xaa);
        assert_eq!(out[0x1184], 1);
        assert_eq!(out[0x10f8], 0x81);
        assert_eq!(out[0x111e], 0x81);
    }

    #[test]
    fn codes_230() {
        assert_eq!(request_info(Proto::V230, 0x0065), Some((0x20, true)));
        assert_eq!(request_info(Proto::V230, 0x0067), Some((0x10, true)));
        assert_eq!(request_info(Proto::V230, 0x0143), Some((0x1190, true)));
        assert_eq!(request_info(Proto::V230, 0x00ab), Some((0x50, false)));
        assert_eq!(request_info(Proto::V210, 0x00ab), Some((0x50, true)));
        assert_eq!(request_info(Proto::V230, 0x7777), None);
        assert_eq!(name(0x0064), "createProfile2");
        // Ver.1's codes stay Ver.1's
        assert_eq!(super::super::request_info(Proto::V1, 0x0067), Some((0x20, true)));
        assert_eq!(super::super::name(Proto::V1, 0x0066), "createProfile");
    }

    #[test]
    fn server_list_230() {
        let out = server_list_at("192.168.1.1", 2);
        assert_eq!(r16(&out, 0x04), 1);
        assert_eq!(&out[6..17], b"192.168.1.1");
        assert_eq!(r16(&out, 0x86), 10000);
        assert_eq!(r16(&out, 0x3a0), 10011);
        assert!(out[0x3a2..].starts_with(b"http://192.168.1.1/idz/news/"));
    }
}
