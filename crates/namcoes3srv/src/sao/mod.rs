//! Sword Art Online Arcade's game server: a port of ARTEMiS' `titles/sao` (WTFPL,
//! gitea.tendokyu.moe/Dniel97/artemis), on the game's own protocol definitions (GssSite.dll's
//! `GameConnectProt`, `schema.rs`).
//!
//! The game posts to `<ALL.Net uri>2022060701/proto/if/<category>/<command>.php`; a message is a
//! 40-byte header (command, 6 zero bytes, vendor 5, game 1, version 1, a 16-byte check sum, the
//! body's length) then the body, Blowfish-CBC encrypted (GssSite's key; an 8-byte IV first,
//! zero padded). The answer: a 24-byte header (command + 1, error status, error type, vendor,
//! game, version, the body's length) then the encrypted body.
//!
//! * `wire.rs`: messages by field name, encoded by the game's schema.
//! * `data.rs`: ARTEMiS' CSV data (master data, static tables).
//! * `game.rs`: ARTEMiS' handlers. `db.rs`: the players' data (`sao.json`).

mod data;
mod db;
mod game;
mod master;
mod schema;
mod value;
mod wire;

use std::net::TcpListener;
use std::sync::{Arc, OnceLock};

use blowfish::Blowfish;
use blowfish::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use blowfish::cipher::generic_array::GenericArray;

use crate::log;

/// GssSite.dll's `GameConnectDef.Base_3DES_key` (a Blowfish key despite the name).
const KEY: &[u8] = b"J7k384wEL6XsVpRa2mQn";

/// `datetime.fromtimestamp(0)` as ARTEMiS sends it (`fmt_dt()`).
pub(crate) const EPOCH: &str = "19700101000000";

/// A reward: `CommonRewardType`, `CommonRewardId`, `CommonRewardNum`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reward {
    pub(crate) kind: i64,
    pub(crate) id: i64,
    pub(crate) num: i64,
}

static SERVER: OnceLock<String> = OnceLock::new();

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A Unix time as `%Y%m%d%H%M%S` (UTC).
pub(crate) fn format_time(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86400), t.rem_euclid(86400));
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}{m:02}{d:02}{:02}{:02}{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// `%Y%m%d%H%M%S` as a Unix time.
pub(crate) fn parse_time(s: &str) -> Option<i64> {
    if s.len() != 14 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<i64>().unwrap_or(0);
    let (y, m, d) = (n(0..4), n(4..6), n(6..8));
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86400 + n(8..10) * 3600 + n(10..12) * 60 + n(12..14))
}

/// A pseudo-random number (Python's `random` in ARTEMiS: choices, card serials).
pub(crate) fn random() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    x
}

fn cipher() -> Blowfish {
    Blowfish::new_from_slice(KEY).expect("blowfish key")
}

/// Blowfish-CBC of `data` (zero padded to 8 bytes) with `iv`.
fn encrypt(data: &[u8], iv: [u8; 8]) -> Vec<u8> {
    let bf = cipher();
    let mut out = Vec::with_capacity(data.len() + 16);
    out.extend_from_slice(&iv);
    let mut prev = iv;
    for chunk in data.chunks(8) {
        let mut block = [0u8; 8];
        block[..chunk.len()].copy_from_slice(chunk);
        for i in 0..8 {
            block[i] ^= prev[i];
        }
        let mut b = GenericArray::from(block);
        bf.encrypt_block(&mut b);
        prev = b.into();
        out.extend_from_slice(&prev);
    }
    out
}

/// The body of `data` (an 8-byte IV, then Blowfish-CBC blocks).
fn decrypt(data: &[u8]) -> Vec<u8> {
    if data.len() < 8 {
        return Vec::new();
    }
    let bf = cipher();
    let mut prev: [u8; 8] = data[..8].try_into().unwrap();
    let mut out = Vec::with_capacity(data.len());
    for chunk in data[8..].chunks(8) {
        let mut block = [0u8; 8];
        block[..chunk.len()].copy_from_slice(chunk);
        let mut b = GenericArray::from(block);
        bf.decrypt_block(&mut b);
        for i in 0..8 {
            out.push(b[i] ^ prev[i]);
        }
        prev = block;
    }
    out
}

/// The answer to a request's bytes: its header and encrypted body (`None`: not a message).
fn answer(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() < 40 {
        log(&format!("sao: short request ({} bytes)", raw.len()));
        return None;
    }
    let cmd = u16::from_be_bytes([raw[0], raw[1]]);
    let body = decrypt(&raw[40..]);
    let name = wire::message(cmd);
    let req = name.map(|n| wire::decode(n, &body)).unwrap_or_default();
    if crate::verbose() {
        log(&format!("sao: {cmd:#06x} {}: {}", name.unwrap_or("?"), {
            let mut s = String::new();
            req.to_json(&mut s);
            s.chars().take(600).collect::<String>()
        }));
    }
    let server = SERVER.get().map_or("127.0.0.1:9002", |s| s.as_str());
    let answer = game::handle(cmd, &req, server);
    let reply = cmd.wrapping_add(1);
    let body = match wire::message(reply) {
        Some(n) => wire::encode(n, &answer.value),
        // no answer message: ARTEMiS' generic one (result 1, an int 0)
        None => vec![1, 0, 0, 0, 0],
    };
    let iv = random().to_le_bytes();
    let crypted = encrypt(&body, iv);
    let mut out = Vec::with_capacity(24 + crypted.len());
    out.extend_from_slice(&reply.to_be_bytes());
    out.extend_from_slice(&answer.err_status.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // error type
    out.extend_from_slice(&5u32.to_be_bytes()); // vendor
    out.extend_from_slice(&1u32.to_be_bytes()); // game
    out.extend_from_slice(&1u32.to_be_bytes()); // version
    out.extend_from_slice(&(crypted.len() as u32).to_be_bytes());
    out.extend_from_slice(&crypted);
    Some(out)
}

fn route(method: &str, target: &str, body: &[u8]) -> Option<Vec<u8>> {
    let path = target.split('?').next().unwrap_or(target);
    if path.ends_with("/system_status.php") {
        return Some(b"open".to_vec());
    }
    if method == "POST" && path.contains("/proto/if/") {
        return answer(body);
    }
    log(&format!("sao: {method} {target}"));
    None
}

/// Starts the server on `addr` (`host:port`, plain HTTP; TLS too), the players' data in `data_dir`.
pub fn start(addr: &str, data_dir: &str, logger: fn(&str), verbose: bool) {
    let _ = crate::LOGGER.set(logger);
    let _ = crate::VERBOSE.set(verbose);
    let _ = SERVER.set(addr.to_string());
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        log(&format!("sao: {data_dir}: {e}"));
    }
    db::open(data_dir);
    let config = match crate::tls_config() {
        Ok(c) => Arc::new(c),
        Err(e) => return log(&format!("sao: TLS: {e}")),
    };
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => return log(&format!("sao: {addr}: {e}")),
    };
    log(&format!("sao: game server on http://{addr}, data in {data_dir}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let config = config.clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::serve(stream, config, route, "application/octet-stream") {
                    log(&format!("sao: connection: {e}"));
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crypt_round_trip() {
        let data = b"hello sword art online".to_vec();
        let c = encrypt(&data, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(c.len(), 8 + 24);
        assert_eq!(&decrypt(&c)[..data.len()], &data[..]);
    }

    #[test]
    fn times() {
        assert_eq!(format_time(0), "19700101000000");
        assert_eq!(format_time(1_700_000_000), "20231114221320");
        assert_eq!(parse_time("20231114221320"), Some(1_700_000_000));
    }

    #[test]
    fn login_flow() {
        db::open(&std::env::temp_dir().join(format!("saosrv-test-{}", std::process::id())).to_string_lossy());
        let login = V::obj().with("access_code", "01234567890123456789").with("chip_id", "0102030405060708");
        let a = game::handle(0xc104, &login, "x");
        assert_eq!(a.value.i("first_play_flag"), 1);
        let user = a.value.i("user_id");
        let req = V::obj().with("user_id", user.to_string());
        let heroes = game::handle(0xc600, &req, "x").value;
        assert_eq!(heroes.get("hero_log_user_data_list").list().len(), 3);
        let parties = game::handle(0xc804, &req, "x").value;
        assert_eq!(parties.get("party_data_list").list()[0].get("party_team_data_list").list().len(), 3);
        let basic = game::handle(0xc500, &req, "x").value;
        assert_eq!(basic.get("user_basic_data").list()[0].s("nick_name"), "PLAYER");
        let bytes = wire::encode("get_user_basic_data_R", &basic);
        assert!(bytes.len() > 40);
        let master = game::handle(0xd520, &V::obj(), "x").value;
        assert!(master.get("hero_log_data_list").list().len() > 100 || !master.get("result").is_null());
    }

    use value::V;
}
