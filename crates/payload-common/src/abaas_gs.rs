//! The ABAAS game server ("GS", `am::abaas::GsMain`: the player data of Blade Strangers and the
//! other APM3 games, Sega World Drivers Championship): HTTP POSTs to `http://<GS server
//! name>/api/...` (user/load, user/save, user/heartbeat, ranking/list|archive|aggregate,
//! data/load|save, server/alive, log/upload, code/convert|create), served by the system's
//! payload ([`reply`] to each `/api/` request). [`configure`] sets the folder of the saved data
//! and the prefix of the options (`<prefix>_GS_KEY`, `<prefix>_GS_SEQ`, `<prefix>_GS_ONLINE`).
//!
//! Bodies: a 16-byte header that is also the IV (`01 01 01 00`, the game id, 8 random bytes),
//! then the zlib-compressed JSON, AES-256-CBC encrypted (PKCS#7) with the title key the game gives
//! the client (`setTitleServerConfig`: learnt at run time, [`KEY`]). Requests:
//! `{"client":{"serial":..,"game_id":..,"game_ver":..,"location_id":..,"create_time":..}, ...}`.

use std::sync::OnceLock;

use aes::Aes256;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};

use crate::log;

/// The saved data's folder and the options' prefix.
static CONFIG: OnceLock<(String, String)> = OnceLock::new();

/// The folder of the players' files and rankings (relative to the process's directory) and the
/// prefix of the options (`WAL_APM3`: `WAL_APM3_GS_KEY`...).
pub fn configure(dir: &str, env_prefix: &str) {
    let _ = CONFIG.set((dir.to_string(), env_prefix.to_string()));
}

/// The title's own data (`data/load`'s `free_buckets` the cabinet has not saved): a key's JSON
/// value, from the system's payload.
static TITLE_DATA: OnceLock<fn(&str) -> Option<String>> = OnceLock::new();

/// Sets the title data the server gives for the keys never saved (Sega World Drivers
/// Championship's seasons and events, which its servers held).
pub fn set_title_data(data: fn(&str) -> Option<String>) {
    let _ = TITLE_DATA.set(data);
}

fn dir() -> &'static str {
    CONFIG.get().map_or("gs", |c| c.0.as_str())
}

/// The option `<prefix>_<name>` (trimmed, unset when empty).
fn env(name: &str) -> Option<String> {
    let prefix = CONFIG.get().map_or("WAL", |c| c.1.as_str());
    std::env::var(format!("{prefix}_{name}")).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The title key the game gives the client (`setTitleServerConfig`), learnt from its
/// `BCryptGenerateSymmetricKey` call (a 256-bit secret); `<prefix>_GS_KEY` (hex) instead.
static KEY: std::sync::Mutex<Option<[u8; 32]>> = std::sync::Mutex::new(None);

pub fn learn_key(secret: &[u8]) {
    let Ok(key) = <[u8; 32]>::try_from(secret) else { return };
    let mut current = KEY.lock().unwrap();
    if env("GS_KEY").is_none() && *current != Some(key) {
        log!("gs: title key {}", key.iter().map(|b| format!("{b:02x}")).collect::<String>());
        *current = Some(key);
    }
}

fn key() -> Option<[u8; 32]> {
    if let Some(hex) = env("GS_KEY") {
        let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
        return bytes.try_into().ok();
    }
    *KEY.lock().unwrap()
}

fn decrypt(iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes256::new(GenericArray::from_slice(&key()?));
    let mut prev: [u8; 16] = iv.try_into().ok()?;
    let mut out = Vec::with_capacity(data.len());
    for block in data.chunks(16) {
        let mut b = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut b);
        out.extend(b.iter().zip(prev).map(|(x, p)| x ^ p));
        prev.copy_from_slice(block);
    }
    let pad = *out.last()? as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return None;
    }
    out.truncate(out.len() - pad);
    Some(out)
}

fn encrypt(iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let cipher = Aes256::new(GenericArray::from_slice(&key().unwrap_or_default()));
    let pad = 16 - data.len() % 16;
    let mut plain = data.to_vec();
    plain.extend(std::iter::repeat_n(pad as u8, pad));
    let mut prev = *iv;
    let mut out = Vec::with_capacity(plain.len());
    for block in plain.chunks(16) {
        let mut b = [0u8; 16];
        for i in 0..16 {
            b[i] = block[i] ^ prev[i];
        }
        let mut ga = GenericArray::from(b);
        cipher.encrypt_block(&mut ga);
        prev.copy_from_slice(&ga);
        out.extend_from_slice(&ga);
    }
    out
}

/// A request's JSON text.
fn open(body: &[u8]) -> Option<String> {
    if body.len() < 32 {
        return None;
    }
    let compressed = decrypt(&body[..16], &body[16..])?;
    let json = miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).ok()?;
    Some(String::from_utf8_lossy(&json).into_owned())
}

/// A response body: a header (also the IV) then the JSON, compressed and encrypted.
fn seal(header: &[u8], json: &str) -> Vec<u8> {
    // response header (as libcts'): 01 01, the status (u16 big endian, 0 = success), the data's
    // CRC32, its length (both big endian: the client sizes its buffer with it), 4 more bytes
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&header[..16]);
    iv[0] = 1;
    iv[1] = 1;
    iv[2..4].copy_from_slice(&0u16.to_be_bytes());
    iv[4..8].copy_from_slice(&crc32(json.as_bytes()).to_be_bytes());
    iv[8..12].copy_from_slice(&(json.len() as u32).to_be_bytes());
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(json.as_bytes(), 6);
    let mut out = iv.to_vec();
    out.extend(encrypt(&iv, &compressed));
    out
}

/// The JSON text of `key`'s value: the first `"key":` of `json` (string-aware: a value
/// containing the key's text in a string is skipped).
fn value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let b = json.as_bytes();
    let pattern = format!("\"{key}\":");
    let mut i = 0;
    let mut in_string = false;
    while i < b.len() {
        match b[i] {
            b'\\' if in_string => i += 1,
            b'"' if !in_string && json[i..].starts_with(&pattern) => return Some(scan(json[i + pattern.len()..].trim_start())),
            b'"' => in_string = !in_string,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The JSON value at the start of `text` (an object, array, string or scalar).
fn scan(text: &str) -> &str {
    let b = text.as_bytes();
    let (mut depth, mut in_string, mut i) = (0i32, false, 0);
    while i < b.len() {
        let c = b[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => {
                    in_string = false;
                    if depth == 0 {
                        return &text[..=i];
                    }
                }
                _ => {}
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' if depth == 0 => return text[..i].trim(),
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return &text[..=i];
                    }
                }
                b',' if depth == 0 => return text[..i].trim(),
                _ => {}
            }
        }
        i += 1;
    }
    text.trim()
}

/// The `"key": value` members of a JSON object (`object`'s text), in order.
fn members(object: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = object.trim().strip_prefix('{').unwrap_or("");
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if !rest.starts_with('"') {
            return out;
        }
        let key = scan(rest);
        rest = rest[key.len()..].trim_start();
        let Some(after) = rest.strip_prefix(':') else { return out };
        let value = scan(after.trim_start());
        rest = &after.trim_start()[value.len()..];
        out.push((key.trim_matches('"'), value));
    }
}

/// The cabinet's data (`data/save`'s `free_buckets`).
fn data_path() -> String {
    format!("{}\\gs-data.json", dir())
}

/// A request's `keys` (key patterns).
fn key_patterns(request: &str) -> Vec<&str> {
    let list = value(request, "keys").unwrap_or("[]").trim().trim_start_matches('[').trim_end_matches(']');
    list.split(',').map(|p| p.trim().trim_matches('"')).filter(|p| !p.is_empty()).collect()
}

/// `user/load`'s key patterns: `*` any end, `?` any character (`league*`, `Ch??rank`).
fn matches(pattern: &str, key: &str) -> bool {
    let (p, k) = (pattern.as_bytes(), key.as_bytes());
    match p.iter().position(|&c| c == b'*') {
        Some(star) => k.len() >= star && p[..star].iter().zip(k).all(|(&a, &b)| a == b'?' || a == b),
        None => p.len() == k.len() && p.iter().zip(k).all(|(&a, &b)| a == b'?' || a == b),
    }
}

/// A player's file (`<dir>/gs-<user id>.json`): `{"name": .., "savedata": {..}}` as the game
/// last sent them (`user/save`).
fn path(user_id: &str) -> String {
    format!("{}\\gs-{user_id}.json", dir())
}

/// The elements of a JSON array (`array`'s text).
fn elements(array: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = array.trim().strip_prefix('[').unwrap_or("");
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() || rest.starts_with(']') {
            return out;
        }
        let element = scan(rest);
        if element.is_empty() {
            return out;
        }
        rest = &rest[element.len()..];
        out.push(element);
    }
}

/// A number member of an object (0 when absent).
fn number(object: &str, key: &str) -> i64 {
    value(object, key).and_then(|v| v.trim_matches('"').parse().ok()).unwrap_or(0)
}

/// One ranking's records (`<dir>/gs-ranking-<id>.txt`): a line per player, best score first:
/// user id, score, sub score, then the name and the game's optional data (JSON strings), tab
/// separated.
struct Record {
    user_id: i64,
    score: i64,
    sub_score: i64,
    name: String,
    optional: String,
}

fn ranking_path(id: i64) -> String {
    format!("{}\\gs-ranking-{id}.txt", dir())
}

fn ranking(id: i64) -> Vec<Record> {
    let text = std::fs::read_to_string(ranking_path(id)).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(5, '\t').collect();
            Some(Record {
                user_id: f.first()?.parse().ok()?,
                score: f.get(1)?.parse().ok()?,
                sub_score: f.get(2)?.parse().ok()?,
                name: f.get(3)?.to_string(),
                optional: f.get(4)?.to_string(),
            })
        })
        .collect()
}

/// `user/save`'s scores (`rankings`: ranking id, score, sub score, optional data): the
/// player's best one kept in each ranking.
fn save_rankings(user_id: i64, name: &str, request: &str) {
    let Some(list) = value(request, "rankings") else { return };
    for entry in elements(list) {
        let id = number(entry, "ranking_id");
        let score = number(entry, "score");
        let mut records = ranking(id);
        if records.iter().any(|r| r.user_id == user_id && r.score >= score) {
            continue;
        }
        records.retain(|r| r.user_id != user_id);
        records.push(Record {
            user_id,
            score,
            sub_score: number(entry, "sub_score"),
            name: name.to_string(),
            optional: value(entry, "optional_data").unwrap_or(r#""""#).to_string(),
        });
        records.sort_by(|a, b| b.score.cmp(&a.score).then(b.sub_score.cmp(&a.sub_score)));
        records.truncate(1000);
        let text: String = records
            .iter()
            .map(|r| format!("{}\t{}\t{}\t{}\t{}\n", r.user_id, r.score, r.sub_score, r.name, r.optional))
            .collect();
        let _ = std::fs::create_dir_all(dir());
        match std::fs::write(ranking_path(id), text) {
            Ok(()) => log!("gs: ranking {id}: player {user_id} score {score}"),
            Err(e) => log!("gs: ranking {id}: {e}"),
        }
    }
}

/// `ranking/list` (`rankings`: ranking id, count): each ranking's best records.
fn list_rankings(request: &str) -> String {
    let lists: Vec<String> = value(request, "rankings")
        .map(elements)
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let id = number(entry, "ranking_id");
            let count = match number(entry, "count") {
                0 => 100,
                n => n,
            };
            let records: Vec<String> = ranking(id)
                .iter()
                .take(count as usize)
                .enumerate()
                .map(|(i, r)| {
                    format!(
                        r#"{{"rank":{},"user_id":{},"name":{},"score":{},"sub_score":{},"optional_data":{}}}"#,
                        i + 1,
                        r.user_id,
                        r.name,
                        r.score,
                        r.sub_score,
                        r.optional
                    )
                })
                .collect();
            format!(r#"{{"ranking_id":{id},"total":{},"records":[{}]}}"#, records.len(), records.join(","))
        })
        .collect();
    format!(r#"{{"rankings":[{}]}}"#, lists.join(","))
}

/// The player's place in the requested rankings (`user.rankings`: ranking ids).
fn player_rankings(user_id: i64, list: &str) -> String {
    let entries: Vec<String> = elements(list)
        .iter()
        .map(|entry| {
            let id = number(entry, "ranking_id");
            let records = ranking(id);
            let (rank, score, sub, optional) = match records.iter().position(|r| r.user_id == user_id) {
                Some(i) => (i + 1, records[i].score, records[i].sub_score, records[i].optional.as_str()),
                None => (0, 0, 0, r#""""#),
            };
            format!(
                r#"{{"ranking_id":{id},"score":{score},"sub_score":{sub},"filter1":0,"filter2":0,"filter3":0,"optional_data":{optional},"rank":{rank},"total":{}}}"#,
                records.len()
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// `user/load`: the player, with the saved values of the requested keys (`user.savedata.keys`;
/// an empty object for a new player: the game starts one) and its place in the requested
/// rankings (`user.rankings`).
fn load(id: &str, request: &str) -> String {
    let file = std::fs::read_to_string(path(id)).ok();
    let mut user = vec![format!(r#""user_id":{id}"#)];
    let keys = value(request, "keys");
    match &file {
        Some(file) => user.push(format!(r#""name":{}"#, value(file, "name").unwrap_or(r#""""#))),
        // no name (and an empty savedata object) for a new player: Senxin Aleste's and Puyo Puyo
        // e-Sports' readers require a name string
        None => user.push(r#""name":"""#.into()),
    }
    // the game's player check (`control.start_game`) asks for no keys: no values
    if keys.is_some() {
        let saved = file.as_deref().and_then(|f| value(f, "savedata")).map(members).unwrap_or_default();
        let patterns = key_patterns(request);
        let data: Vec<String> = saved
            .iter()
            .filter(|(key, _)| patterns.iter().any(|p| matches(p, key)))
            .map(|(key, v)| format!(r#""{key}":{v}"#))
            .collect();
        log!("gs: player {id}: {}, {} of {} values", if file.is_some() { "known" } else { "new" }, data.len(), saved.len());
        user.push(format!(r#""savedata":{{{}}}"#, data.join(",")));
    }
    if let Some(list) = value(request, "rankings") {
        user.push(format!(r#""rankings":{}"#, player_rankings(id.parse().unwrap_or(0), list)));
    }
    format!(r#"{{"user":{{{}}}}}"#, user.join(","))
}

/// The JSON answer to one request (`api`: the path after `/api/`).
fn answer(api: &str, request: &str) -> Option<String> {
    let user_id = value(request, "user_id").filter(|id| id.bytes().all(|c| c.is_ascii_digit()));
    match (api, user_id) {
        // the game's online flag: control.online nonzero
        ("server/alive", _) => Some(r#"{"control":{"online":1}}"#.into()),
        ("user/load", Some(id)) => Some(load(id, request)),
        // the end of a game: the name and all values (a second save without them: nothing),
        // the scores
        ("user/save", Some(id)) => {
            // the values sent merged into the saved ones, the saved name kept when none is sent
            // (Sega World Drivers Championship ends a play with two saves: the name and 9 values,
            // then 129 other values without the name)
            let old = std::fs::read_to_string(path(id)).unwrap_or_default();
            let name = value(request, "name").or_else(|| value(&old, "name")).unwrap_or(r#""""#).to_string();
            if let Some(data) = value(request, "savedata") {
                let mut merged: Vec<(String, String)> = value(&old, "savedata").map(members).unwrap_or_default().into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                for (k, v) in members(data) {
                    match merged.iter_mut().find(|(m, _)| m == k) {
                        Some(entry) => entry.1 = v.to_string(),
                        None => merged.push((k.to_string(), v.to_string())),
                    }
                }
                let data = format!("{{{}}}", merged.iter().map(|(k, v)| format!(r#""{k}":{v}"#)).collect::<Vec<_>>().join(","));
                let _ = std::fs::create_dir_all(dir());
                match std::fs::write(path(id), format!(r#"{{"name":{name},"savedata":{data}}}"#)) {
                    Ok(()) => log!("gs: player {id}: data saved ({} values)", merged.len()),
                    Err(e) => log!("gs: player {id}: {e}"),
                }
            }
            save_rankings(id.parse().unwrap_or(0), &name, request);
            Some(format!(r#"{{"user":{{"user_id":{id}}}}}"#))
        }
        ("ranking/list", _) => Some(list_rankings(request)),
        // the cabinet's own data (`free_buckets`: Pengo's settings), all players'
        ("data/load", _) => {
            let saved = std::fs::read_to_string(data_path()).unwrap_or_default();
            let saved = members(if saved.is_empty() { "{}" } else { &saved }).into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
            let patterns = key_patterns(request);
            let mut data: Vec<String> =
                saved.iter().filter(|(k, _)| patterns.iter().any(|p| matches(p, k))).map(|(k, v)| format!(r#""{k}":{v}"#)).collect();
            // the title's data for the keys (no wildcard) never saved
            if let Some(title) = TITLE_DATA.get() {
                for key in patterns.iter().filter(|p| !p.contains(['*', '?']) && !saved.iter().any(|(k, _)| k == *p)) {
                    if let Some(v) = title(key) {
                        data.push(format!(r#""{key}":{v}"#));
                    }
                }
            }
            Some(format!(r#"{{"free_buckets":{{{}}}}}"#, data.join(",")))
        }
        ("data/save", _) => {
            if let Some(buckets) = value(request, "free_buckets") {
                let old = std::fs::read_to_string(data_path()).unwrap_or_default();
                let mut merged: Vec<(String, String)> =
                    members(if old.is_empty() { "{}" } else { &old }).into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                for (k, v) in members(buckets) {
                    match merged.iter_mut().find(|(m, _)| m == k) {
                        Some(entry) => entry.1 = v.to_string(),
                        None => merged.push((k.to_string(), v.to_string())),
                    }
                }
                let text = format!("{{{}}}", merged.iter().map(|(k, v)| format!(r#""{k}":{v}"#)).collect::<Vec<_>>().join(","));
                let _ = std::fs::create_dir_all(dir());
                match std::fs::write(data_path(), text) {
                    Ok(()) => log!("gs: cabinet data saved ({} values)", merged.len()),
                    Err(e) => log!("gs: cabinet data: {e}"),
                }
            }
            Some("{}".into())
        }
        _ => Some("{}".into()),
    }
}

/// The reply body to one request (`path` from `/api/`).
pub fn reply(path: &str, body: &[u8]) -> Vec<u8> {
    let api = path.trim_start_matches("/api/");
    let Some(request) = open(body) else {
        log!("gs: {api}: {} bytes, not readable", body.len());
        return Vec::new();
    };
    log!("gs: {api} {request}");
    match answer(api, &request) {
        // `control` in every answer (Senxin Aleste reads `online` from each, an exception
        // without it): the server's for server/alive, the player's elsewhere (1: still playing
        // on another cabinet, "the last play did not end normally")
        Some(json) if value(&json, "control").is_none() => {
            // a play's sequence id and token (strings), given when a play starts (`user/load`
            // with `control.start_game` 1: Puyo Puyo e-Sports reads `seq_id` there; the games send
            // them back). `<prefix>_GS_SEQ=0`: never (Nosferatu Lilinor refuses the player's data
            // with one: "the play data could not be read")
            static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
            let starts = value(&request, "start_game").is_some_and(|v| v.trim() == "1")
                && env("GS_SEQ").is_none_or(|v| v != "0");
            // `<prefix>_GS_ONLINE=1`: the player's `online` 1 in `user/load` (Dead or Alive 6: the
            // card player then online, else no heartbeat and no `user/save` at the end of play)
            let online = u8::from(api == "user/load" && env("GS_ONLINE").is_some_and(|v| v == "1"));
            let control = if starts {
                let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                format!(r#""control":{{"online":{online},"seq_id":"{seq}","token":"{seq:08x}"}}"#)
            } else {
                format!(r#""control":{{"online":{online}}}"#)
            };
            let json = match json.strip_suffix('}') {
                Some(body) if body.len() > 1 => format!("{body},{control}}}"),
                _ => format!("{{{control}}}"),
            };
            log!("gs: {api} -> {json}");
            seal(body, &json)
        }
        Some(json) => {
            log!("gs: {api} -> {json}");
            seal(body, &json)
        }
        None => Vec::new(),
    }
}
