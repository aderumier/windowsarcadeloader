//! The game server of Guilty Gear -Strive- (Arc System Works' `TLGameServerAPI`, its own, not
//! SEGA's): WinINet POSTs of `data=<hex MessagePack>` to `<base>/api/<name>`, the base patched by
//! the profile to `http://127.0.0.1/api/` (the payload's title server, [`crate::apms`]).
//!
//! A request: `[[user id, session, language, version, platform], param]`. A response:
//! `[[session, result, date, appVer, dataVer, serverVer], param]`. Positional arrays; the game's
//! MessagePack structs take shorter ones, the rest left empty: `[0]` (status 0) answers every API
//! without data here (news, rankings, items, VIP, results).
//!
//! The player: `user/login` with the Aime id, whose answer's user id and session the next
//! requests send. The player's data is the game's `statistics/ac_get` / `ac_set` blobs, one per
//! data type, character and matching value: `apms/tl-<user id>-<type>-<chara>-<value>.bin`; and
//! its compressed save file, `tus/write` / `tus/read`: `apms/tl-<user id>-tus-<slot>.bin`. A known
//! player's `tus/read` without a file answers status 1 (status 0 with no data: the game fatally
//! fails to decompress it, "BulkData compressed header read error").

use wal_payload_common::log;

use crate::cts::{self, Value, arr, int, s};

fn unhex(text: &[u8]) -> Option<Vec<u8>> {
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    text.chunks_exact(2).map(|p| Some(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

/// `apms/tl-...` of a player's blob; only digits and letters of the user id, the game's text.
fn blob_path(user: &str, kind: i64, chara: i64, value: i64) -> String {
    let user: String = user.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("apms/tl-{user}-{kind}-{chara}-{value}.bin")
}

fn tus_path(user: &str, slot: i64) -> String {
    let user: String = user.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("apms/tl-{user}-tus-{slot}.bin")
}

fn user_info(id: &str, name: &[u8]) -> Value {
    arr(vec![s(id), Value::Str(name.to_vec()), s(id), s(""), int(1), int(0)])
}

fn text(v: &Value) -> String {
    String::from_utf8_lossy(v.bytes().unwrap_or_default()).into_owned()
}

/// The param of a response.
fn answer(api: &str, user: &str, param: &Value) -> Value {
    match api {
        "sys/get_env" => arr(vec![int(0), s("http://127.0.0.1/api/")]),
        // [sub platform, Aime id, ...]: every card has a player, the data is new or kept
        "user/login" => {
            let id = text(param.get(1));
            arr(vec![int(0), user_info(&id, crate::env("WAL_APM3_PLAYER_NAME").unwrap_or_else(|| "PLAYER".into()).as_bytes())])
        }
        // [user id, type, mode, chara, opponent, value]; none: the game makes a new player's
        "statistics/ac_get" => {
            let target = Some(text(param.get(0))).filter(|t| !t.is_empty()).unwrap_or_else(|| user.to_string());
            let at = param.array();
            let number = |i: usize| at.get(i).and_then(Value::int).unwrap_or(-1);
            match std::fs::read(blob_path(&target, number(1), number(3), number(5))) {
                Ok(blob) => arr(vec![int(0), Value::Str(blob)]),
                Err(_) => arr(vec![int(0), s("")]),
            }
        }
        // [type, chara, value, blob]
        "statistics/ac_set" => {
            let number = |i: usize| param.get(i).int().unwrap_or(-1);
            let _ = std::fs::create_dir_all("apms");
            if let Some(blob) = param.get(3).bytes().filter(|_| !user.is_empty()) {
                let path = blob_path(user, number(0), number(1), number(2));
                if let Err(e) = std::fs::write(&path, blob) {
                    log!("tl: {path}: {e}");
                }
            }
            arr(vec![int(0)])
        }
        // [user id, slot]
        "tus/read" => {
            let target = Some(text(param.get(0))).filter(|t| !t.is_empty()).unwrap_or_else(|| user.to_string());
            match std::fs::read(tus_path(&target, param.get(1).int().unwrap_or(0))) {
                Ok(data) if !data.is_empty() => arr(vec![int(0), Value::Str(data)]),
                _ => arr(vec![int(1)]),
            }
        }
        // [slot, data]
        "tus/write" => {
            let _ = std::fs::create_dir_all("apms");
            if let Some(data) = param.get(1).bytes().filter(|_| !user.is_empty()) {
                let path = tus_path(user, param.get(0).int().unwrap_or(0));
                if let Err(e) = std::fs::write(&path, data) {
                    log!("tl: {path}: {e}");
                }
            }
            arr(vec![int(0)])
        }
        _ => arr(vec![int(0)]),
    }
}

/// `YYYY/MM/DD hh:mm.ss`, UTC.
fn date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, day_secs) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil from days (proleptic Gregorian)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}/{m:02}/{d:02} {:02}:{:02}.{:02}", day_secs / 3600, day_secs / 60 % 60, day_secs % 60)
}

/// A request's response bytes (`data=<hex>` from the game).
pub(crate) fn reply(path: &str, body: &[u8]) -> Vec<u8> {
    let api = path.strip_prefix("/api/").unwrap_or(path);
    let hex = body.strip_prefix(b"data=").unwrap_or(body);
    let raw = unhex(hex.split(|&b| b == 0 || b == b'&').next().unwrap_or(hex)).unwrap_or_default();
    let request = cts::decode_all(&raw).into_iter().next().unwrap_or(Value::Nil);
    log!("tl: {api} {}", cts::show(&request));
    let header = request.get(0);
    let user = text(header.get(0));
    let param = answer(api, &user, request.get(1));
    log!("tl: {api} -> {}", cts::show(&param));
    let version = header.get(3).clone();
    let session = if api == "user/login" { Value::Str(format!("s{}", text(request.get(1).get(1))).into_bytes()) } else { header.get(1).clone() };
    arr(vec![arr(vec![session, int(0), s(&date()), version.clone(), version.clone(), version]), param]).encode()
}
