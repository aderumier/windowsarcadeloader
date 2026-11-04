//! JoJo's Bizarre Adventure: Last Survivor's game server (the game's `AJBNetAPI` over its
//! NetCommon plugin's `UNetHttp`), run in the game's process by its payload: the game posts
//! to `https://<ALL.Net uri>/<api>` (`sys/serveralive`, `sys/shop_info`, `user/check`, ...).
//!
//! A message (request and answer, `application/x-msgpack`) is a 16-byte hash then
//! MessagePack; the hash is the MD5 of the MessagePack and the request's hash key (`AJB `
//! before a login). A request is `[header, parameters]`, the header a map (`language`,
//! `platform`, `session`, `userId`, `version`); an answer `[header, [result, values...]]`, the
//! header a map (`result`, `session`: the player's session, set by a login), result 0 a
//! success (`Response_<Api>` reads the values by position).

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::log;
use crate::msgpack::Value;

/// The hash key of the requests made before a player logs in.
const HASH_KEY: &[u8] = b"AJB ";

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Starts the server on `addr` (`host:port`), the players' data in `data_dir`. `verbose`:
/// every request and answer in the log.
pub fn start(addr: &str, data_dir: &str, logger: fn(&str), verbose: bool) {
    let _ = crate::LOGGER.set(logger);
    let _ = crate::VERBOSE.set(verbose);
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        log(&format!("jojo: {data_dir}: {e}"));
    }
    let _ = DIR.set(PathBuf::from(data_dir));
    let config = match crate::tls_config() {
        Ok(c) => Arc::new(c),
        Err(e) => return log(&format!("jojo: TLS: {e}")),
    };
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => return log(&format!("jojo: {addr}: {e}")),
    };
    log(&format!("jojo: listening on https://{addr}, data in {data_dir}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let config = config.clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::serve(stream, config, route, "application/x-msgpack") {
                    log(&format!("jojo: connection: {e}"));
                }
            });
        }
    });
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// A message: the hash of `body` with `key`, then `body`.
fn message(body: &Value, key: &[u8]) -> Vec<u8> {
    let body = body.encode();
    let mut out = md5(&[body.as_slice(), key].concat()).to_vec();
    out.extend(body);
    out
}

/// A login's values (`user/create`, `user/login`): the player (16 values by position: user
/// data id, a string, 3 numbers (+ EXP, tickets), a string, a number, a string, 5 numbers,
/// custom data, 2 numbers), the sync server host and 3 numbers. A guest (`user/create`) gets
/// no session: the game then plays offline (no national or store battle).
fn user(params: &Value) -> Vec<Value> {
    let name = params.as_array().first().and_then(Value::as_str).unwrap_or("").to_string();
    let player = vec![
        Value::str("USER0001"),
        Value::str(""),
        Value::Int(0),
        Value::Int(0),
        Value::Int(0),
        Value::str(""),
        Value::Int(0),
        Value::str(""),
        Value::Int(0),
        Value::Int(0),
        Value::Int(0),
        Value::Int(0),
        Value::Int(0),
        Value::Array(vec![]),
        Value::Int(0),
        Value::Int(0),
    ];
    log(&format!("jojo: player {name}"));
    vec![Value::Array(player), Value::str("127.0.0.1"), Value::Int(9003), Value::Int(0), Value::Int(0)]
}

fn route(method: &str, target: &str, body: &[u8]) -> Option<Vec<u8>> {
    let api = target.trim_start_matches('/').split('?').next().unwrap_or("");
    let Some((request, _)) = body.get(16..).and_then(Value::decode) else {
        log(&format!("jojo: {method} {target}: not decoded: {}", hex(body)));
        return None;
    };
    if md5(&[&body[16..], HASH_KEY].concat()) != body[..16] {
        log(&format!("jojo: {api}: hash with another key"));
    }
    let header = &request.as_array().first().cloned().unwrap_or(Value::Nil);
    let params = request.as_array().get(1).cloned().unwrap_or(Value::Nil);
    if crate::verbose() {
        log(&format!("jojo: {api} {header:?} {params:?}"));
    }
    let values = match api {
        // the alive check's period (s), the offline period (start, end)
        "sys/serveralive" => vec![Value::Int(60), Value::str(""), Value::str("")],
        // the shop's status, 0: a valid shop id (the game instance's IsValidShopId)
        "sys/shop_info" => vec![Value::Int(0)],
        "user/create" | "user/login" => user(&params),
        // the card's lock (s) from now: the game extends it every 26 s, out of time it leaves
        // ("disconnected from the game server")
        "user/extend_bp_lock" => vec![Value::Int(180)],
        // the national battles' periods: [play mode (EPlayMode: 1 solo, 2 pair), start, end]
        "sys/national_match_info" => vec![Value::Array(
            [1, 2].iter().map(|&mode| Value::Array(vec![Value::Int(mode), Value::str("2020-01-01 00:00:00"), Value::str("2099-12-31 23:59:59")])).collect(),
        )],
        _ => {
            // the answers read their values by position, missing ones as defaults
            log(&format!("jojo: {api}: no values ({params:?})"));
            vec![]
        }
    };
    // the header: the player's session (a login's, else the request's)
    let session = match api {
        "user/create" | "user/login" => format!("S{:016x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)),
        _ => header.get("session").and_then(Value::as_str).unwrap_or("").to_string(),
    };
    let header = Value::Map(vec![(Value::str("result"), Value::Int(0)), (Value::str("session"), Value::Str(session))]);
    let answer = Value::Array(vec![header, Value::Array([vec![Value::Int(0)], values].concat())]);
    if crate::verbose() {
        log(&format!("jojo: {api} -> {answer:?}"));
    }
    Some(message(&answer, HASH_KEY))
}

/// MD5 (RFC 1321).
fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_le_bytes());
    let mut h = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g]).rotate_left(S[i]));
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 16];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// The sync server: the `UNetSocket` connection of a logged-in player (its host and port in
/// the login answer). A frame is a 54-byte header (`SetHeader`: user id, platform, version,
/// marker; +0x2e the payload size, +0x32 the protocol id) then the payload (AES-128-CBC with
/// the cipher mode on).
pub fn start_sync(addr: &str) {
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => return log(&format!("jojo sync: {addr}: {e}")),
    };
    log(&format!("jojo sync: listening on {addr}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || sync_connection(stream));
        }
    });
}

/// The sync messages' AES-128 key (the game's `BP_AJBOutGameProxy`).
const SYNC_KEY: &[u8; 16] = b"0123456789012345";
const SYNC_HEADER: usize = 54;

/// AES-128-CBC with PKCS#7 padding: `iv` then the ciphertext.
fn sync_encrypt(plain: &[u8]) -> Vec<u8> {
    use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
    let cipher = aes::Aes128::new(GenericArray::from_slice(SYNC_KEY));
    let pad = 16 - plain.len() % 16;
    let mut data = plain.to_vec();
    data.extend(std::iter::repeat_n(pad as u8, pad));
    let mut out = vec![0u8; 16];
    let mut prev = [0u8; 16];
    for chunk in data.chunks(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        for (b, p) in block.iter_mut().zip(prev) {
            *b ^= p;
        }
        cipher.encrypt_block(&mut block);
        prev.copy_from_slice(&block);
        out.extend_from_slice(&block);
    }
    out
}

/// The plaintext of `data` (`iv` then the ciphertext), its padding removed.
fn sync_decrypt(data: &[u8]) -> Option<Vec<u8>> {
    use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};
    if data.len() < 32 || data.len() % 16 != 0 {
        return None;
    }
    let cipher = aes::Aes128::new(GenericArray::from_slice(SYNC_KEY));
    let mut out = Vec::new();
    for (prev, chunk) in data.chunks(16).zip(data[16..].chunks(16)) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        out.extend(block.iter().zip(prev).map(|(b, p)| b ^ p));
    }
    let pad = *out.last()? as usize;
    (1..=16).contains(&pad).then(|| out.truncate(out.len() - pad))?;
    Some(out)
}

fn sync_connection(mut stream: std::net::TcpStream) {
    use std::io::{Read, Write};
    log(&format!("jojo sync: connection from {:?}", stream.peer_addr()));
    loop {
        let mut header = [0u8; SYNC_HEADER];
        if stream.read_exact(&mut header).is_err() {
            return log("jojo sync: closed");
        }
        let size = u32::from_le_bytes(header[0x2e..0x32].try_into().unwrap()) as usize;
        let protocol = u32::from_le_bytes(header[0x32..0x36].try_into().unwrap());
        let mut payload = vec![0u8; size];
        if stream.read_exact(&mut payload).is_err() {
            return log("jojo sync: closed");
        }
        let request = sync_decrypt(&payload).and_then(|p| Value::decode(&p)).map(|(v, _)| v);
        log(&format!("jojo sync: protocol {protocol} {request:?}"));
        // the answer and its protocol id (the game's `Receive_SocketNetMessage`)
        let (id, answer) = match protocol {
            // the server test: result, the matching NPCs' count and [a, b] pairs
            20000 => (51000u32, Value::Array(vec![Value::Int(0), Value::Int(3), Value::Array((1..=3).map(|i| Value::Array(vec![Value::Int(i), Value::Int(1)])).collect())])),
            _ => continue,
        };
        let body = sync_encrypt(&answer.encode());
        let mut out = header.to_vec();
        out[0x2e..0x32].copy_from_slice(&(body.len() as u32).to_le_bytes());
        out[0x32..0x36].copy_from_slice(&id.to_le_bytes());
        out.extend(body);
        if stream.write_all(&out).is_err() {
            return log("jojo sync: closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_messages_decrypt() {
        // the game's first message (protocol 20000): ["123"]
        let payload: Vec<u8> = (0..64).step_by(2).map(|i| u8::from_str_radix(&"000000000000000000000000000000009245aa8f203803cea4aba074acfa8b79"[i..i + 2], 16).unwrap()).collect();
        let plain = sync_decrypt(&payload).unwrap();
        assert_eq!(Value::decode(&plain).unwrap().0, Value::Array(vec![Value::str("123")]));
        assert_eq!(sync_decrypt(&sync_encrypt(b"hello")).unwrap(), b"hello");
    }
}
