//! The players' documents and the teams: one JSON object per Aime user id in
//! `Y:\idz\<id>.json` (minime's `idz_*` tables of one profile: the profile, its chara, cars and
//! selected car, settings, unlocks, tickets, missions, story, course plays, titles, backgrounds,
//! time attack bests; the game's opaque bytes as hex), the teams in `Y:\idz\teams.json`
//! (`teams` with their `members`, the auto teams' `auto` counter, the `reservations` of the
//! players joining an auto team before their profile exists, `next_id`). The server's lock
//! covers a whole request (load, change, save).

use std::sync::Mutex;

use wal_payload_common::json::Json;

/// Held for each request.
pub(super) static LOCK: Mutex<()> = Mutex::new(());

pub(super) const DIR: &str = "Y:\\idz";

fn path(name: &str) -> String {
    format!("{DIR}\\{name}.json")
}

fn read(name: &str) -> Option<Json> {
    std::fs::read_to_string(path(name)).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_)))
}

fn write(name: &str, doc: &Json) {
    let _ = std::fs::create_dir_all(DIR);
    let tmp = format!("{}.tmp", path(name));
    if std::fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path(name));
    }
}

/// The player `aime`'s document.
pub(super) fn load(aime: u32) -> Option<Json> {
    read(&aime.to_string())
}

pub(super) fn save(aime: u32, doc: &Json) {
    write(&aime.to_string(), doc);
}

/// Every player's document (the top ten, the team rosters).
pub(super) fn players() -> Vec<Json> {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir(DIR) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json").and_then(|n| n.parse::<u32>().ok())
                && let Some(doc) = load(id)
            {
                out.push(doc);
            }
        }
    }
    out
}

/// The teams' document (created empty).
pub(super) fn load_teams() -> Json {
    read("teams").unwrap_or_else(|| Json::obj().with("next_id", 1u32))
}

pub(super) fn save_teams(doc: &Json) {
    write("teams", doc);
}

/// `doc`'s array `key` (created empty).
pub(super) fn arr<'a>(doc: &'a mut Json, key: &str) -> &'a mut Vec<Json> {
    if !matches!(doc.get(key), Some(Json::Arr(_))) {
        doc.set(key, Vec::<Json>::new());
    }
    match doc.get_mut(key) {
        Some(Json::Arr(items)) => items,
        _ => unreachable!("idz: document is an object"),
    }
}

/// `doc`'s object `key` (created empty).
pub(super) fn obj<'a>(doc: &'a mut Json, key: &str) -> &'a mut Json {
    if !matches!(doc.get(key), Some(Json::Obj(_))) {
        doc.set(key, Json::obj());
    }
    doc.get_mut(key).expect("idz: document is an object")
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `text`'s bytes, `len` of them (zeros past its end).
pub(super) fn unhex(text: &str, len: usize) -> Vec<u8> {
    let mut out: Vec<u8> = text.as_bytes().chunks(2).map_while(|c| std::str::from_utf8(c).ok().and_then(|s| u8::from_str_radix(s, 16).ok())).take(len).collect();
    out.resize(len, 0);
    out
}

/// `doc`'s hex field `key` as `len` bytes.
pub(super) fn bytes(doc: &Json, key: &str, len: usize) -> Vec<u8> {
    unhex(doc.s(key), len)
}
