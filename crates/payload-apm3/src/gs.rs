//! The ABAAS game server ("GS", `am::abaas::GsMain`: Blade Strangers' player data): HTTP POSTs to
//! `http://<AllnetAuth_getAbaasGsServerName>/api/...` (user/load, user/save, user/heartbeat,
//! ranking/list|archive|aggregate, data/load|save, server/alive, log/upload, code/convert|create),
//! served on the payload's title server port (`apms.rs`: the name is 127.0.0.1, port 80 moved
//! there), routed by the `/api/` path.
//!
//! Bodies: a 16-byte header that is also the IV (`01 01 01 00`, the game id, 8 random bytes),
//! then the zlib-compressed JSON, AES-256-CBC encrypted (PKCS#7) with the title key the game gives
//! the client (`setTitleServerConfig`; Blade Strangers' key in [`KEY`]). Requests:
//! `{"client":{"serial":..,"game_id":..,"game_ver":..,"location_id":..,"create_time":..}, ...}`.

use aes::Aes256;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::log;

/// Blade Strangers (APM3)'s title key (`BCryptGenerateSymmetricKey`, `WAL_APM3_APMS_TRACE=1`).
const KEY: [u8; 32] = [
    0x56, 0xbd, 0x36, 0xd0, 0xd0, 0x5c, 0x8d, 0x01, 0x00, 0xa8, 0x90, 0xb5, 0x7f, 0x29, 0x91, 0x98, 0xaf, 0x54, 0xa5, 0x69, 0xcb, 0xb2,
    0xfc, 0xa2, 0xaf, 0x23, 0xbf, 0x62, 0x16, 0x81, 0xc8, 0x09,
];

fn decrypt(iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes256::new(GenericArray::from_slice(&KEY));
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
    let cipher = Aes256::new(GenericArray::from_slice(&KEY));
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
    iv[4..8].copy_from_slice(&crate::cts::crc32(json.as_bytes()).to_be_bytes());
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

/// `user/load`'s key patterns: `*` any end, `?` any character (`league*`, `Ch??rank`).
fn matches(pattern: &str, key: &str) -> bool {
    let (p, k) = (pattern.as_bytes(), key.as_bytes());
    match p.iter().position(|&c| c == b'*') {
        Some(star) => k.len() >= star && p[..star].iter().zip(k).all(|(&a, &b)| a == b'?' || a == b),
        None => p.len() == k.len() && p.iter().zip(k).all(|(&a, &b)| a == b'?' || a == b),
    }
}

/// A player's file (`apms/gs-<user id>.json`): `{"name": .., "savedata": {..}}` as the game
/// last sent them (`user/save`).
fn path(user_id: &str) -> String {
    format!("apms\\gs-{user_id}.json")
}

/// `user/load`: the player, with the saved values of the requested keys (`user.savedata.keys`;
/// none for a new player: the game starts one).
fn load(id: &str, request: &str) -> String {
    let Ok(file) = std::fs::read_to_string(path(id)) else {
        log!("gs: player {id}: new");
        return format!(r#"{{"user":{{"user_id":{id}}}}}"#);
    };
    let name = value(&file, "name").unwrap_or(r#""""#);
    let saved = value(&file, "savedata").map(members).unwrap_or_default();
    // the game's player check (`control.start_game`) asks for no keys: the player only
    let Some(keys) = value(request, "keys") else {
        return format!(r#"{{"user":{{"user_id":{id},"name":{name}}}}}"#);
    };
    let list = keys.trim().trim_start_matches('[').trim_end_matches(']');
    let patterns: Vec<&str> = list.split(',').map(|p| p.trim().trim_matches('"')).filter(|p| !p.is_empty()).collect();
    let data: Vec<String> = saved
        .iter()
        .filter(|(key, _)| patterns.iter().any(|p| matches(p, key)))
        .map(|(key, v)| format!(r#""{key}":{v}"#))
        .collect();
    log!("gs: player {id}: {} of {} values", data.len(), saved.len());
    format!(r#"{{"user":{{"user_id":{id},"name":{name},"savedata":{{{}}}}}}}"#, data.join(","))
}

/// The JSON answer to one request (`api`: the path after `/api/`).
fn answer(api: &str, request: &str) -> Option<String> {
    let user_id = value(request, "user_id").filter(|id| id.bytes().all(|c| c.is_ascii_digit()));
    match (api, user_id) {
        // the game's online flag: control.online nonzero
        ("server/alive", _) => Some(r#"{"control":{"online":1}}"#.into()),
        ("user/load", Some(id)) => Some(load(id, request)),
        // the end of a game: the name and all values (a second save without them: nothing)
        ("user/save", Some(id)) => {
            if let Some(data) = value(request, "savedata") {
                let name = value(request, "name").unwrap_or(r#""""#);
                let _ = std::fs::create_dir_all("apms");
                match std::fs::write(path(id), format!(r#"{{"name":{name},"savedata":{data}}}"#)) {
                    Ok(()) => log!("gs: player {id}: data saved ({} bytes)", data.len()),
                    Err(e) => log!("gs: player {id}: {e}"),
                }
            }
            Some(format!(r#"{{"user":{{"user_id":{id}}}}}"#))
        }
        _ => Some("{}".into()),
    }
}

/// The reply body to one request (`path` from `/api/`).
pub(crate) fn reply(path: &str, body: &[u8]) -> Vec<u8> {
    let api = path.trim_start_matches("/api/");
    let Some(request) = open(body) else {
        log!("gs: {api}: {} bytes, not readable", body.len());
        return Vec::new();
    };
    log!("gs: {api} {request}");
    match answer(api, &request) {
        Some(json) => {
            log!("gs: {api} -> {json}");
            seal(body, &json)
        }
        None => Vec::new(),
    }
}
