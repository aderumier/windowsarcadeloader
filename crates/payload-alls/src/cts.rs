//! Sega's CTS game server (`libcts` 2.51, Mario & Sonic at the Rio 2016 Olympic Games): the game
//! asks `http://<ALL.Net's host>/apms-api/<request>` through libcurl (`Alive`, `GetRanking`,
//! `SetCabinetProperty`, `StartGame`, `EndGame`, `UpdateUser`...). Its connections to port 80 of
//! the virtual router go to this server in the game's process, 127.0.0.1:`WAL_PORT`+104
//! (`allnet.rs`). Found in the game's code (`am::cts::Client`, `AppData`).
//!
//! Bodies, both ways: a 16-byte header, then the message: MessagePack (the old raw format),
//! zlib compressed, AES-128-CBC encrypted (PKCS#7 padding) with a fixed key, the header as IV. A
//! request's header is `02 01 01 00`, the game id, 8 random bytes; its message `[[keychip id,
//! game id, version, place id, time], body...]`. An answer's header is `02 01`, the result (u16
//! big endian, 0: good), the message's CRC-32 and size (u32 big endian; another CRC: result
//! -201, the request retried), 4 bytes; libcts reads its first MessagePack object (`AppData`'s
//! parsers, all accepting an empty array: no data).
//!
//! - `StartGame` (`[cts id, ""], [start slot, slot count]`; the cts id is the Aime id): the card's
//!   player, `[[cts id, ""], [name, start slot, save]]`, its save the slots asked (1 KB each,
//!   Rio: 2). A new card: no name (the game asks for it) and blank slots (the game's defaults).
//! - `EndGame`, `ContinueGame`, `UpdateUser` (`[cts id, ""], [name, slot, data], play log,
//!   results, session`): the name and the changed slot kept in `Y:\cts\<cts id>.name`, `.bin`
//! - `GetRanking` (`[0, ranking ids, filters]`): one list of entries per ranking id (an entry:
//!   16 fields, rank, score, name...); answered empty (the attract's rankings show no one)
//! - the other requests (`Alive`, `Heartbeat`, `SetCabinetProperty`...): an empty array

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::log;

const KEY: [u8; 16] = [0x8a, 0x8d, 0xf8, 0x0d, 0x93, 0xda, 0x05, 0x2a, 0x14, 0xd0, 0x49, 0x0e, 0xe2, 0x3a, 0x59, 0xd1];

/// A MessagePack value (the subset libcts uses).
#[derive(Clone)]
enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    /// Raw bytes (a raw that is not UTF-8: the player's save data)
    Bin(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Bin(b) => write!(f, "<{} bytes>", b.len()),
            Value::Array(a) => f.debug_list().entries(a).finish(),
            Value::Map(m) => f.debug_map().entries(m.iter().map(|(k, v)| (k, v))).finish(),
        }
    }
}

/// `n` big endian bytes at the start of `r`, and the bytes after them.
fn take(n: usize, r: &[u8]) -> Option<(u64, &[u8])> {
    Some((r.get(..n)?.iter().fold(0u64, |a, &x| a << 8 | x as u64), r.get(n..)?))
}

fn string(n: usize, r: &[u8]) -> Option<(Value, &[u8])> {
    let raw = r.get(..n)?;
    let v = match std::str::from_utf8(raw) {
        Ok(s) => Value::Str(s.into()),
        Err(_) => Value::Bin(raw.to_vec()),
    };
    Some((v, r.get(n..)?))
}

fn array(n: usize, mut r: &[u8]) -> Option<(Value, &[u8])> {
    let mut v = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let (x, rest) = decode(r)?;
        v.push(x);
        r = rest;
    }
    Some((Value::Array(v), r))
}

fn map(n: usize, mut r: &[u8]) -> Option<(Value, &[u8])> {
    let mut v = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let (k, rest) = decode(r)?;
        let (x, rest) = decode(rest)?;
        v.push((k, x));
        r = rest;
    }
    Some((Value::Map(v), r))
}

/// The MessagePack value at the start of `d`, and the bytes after it.
fn decode(d: &[u8]) -> Option<(Value, &[u8])> {
    let (&t, rest) = d.split_first()?;
    match t {
        0x00..=0x7f => Some((Value::Int(t as i64), rest)),
        0x80..=0x8f => map((t & 0x0f) as usize, rest),
        0x90..=0x9f => array((t & 0x0f) as usize, rest),
        0xa0..=0xbf => string((t & 0x1f) as usize, rest),
        0xc0 => Some((Value::Nil, rest)),
        0xc2 | 0xc3 => Some((Value::Bool(t == 0xc3), rest)),
        0xcc..=0xcf => take(1 << (t - 0xcc), rest).map(|(v, r)| (Value::Int(v as i64), r)),
        0xd0..=0xd3 => {
            let n = 1usize << (t - 0xd0);
            take(n, rest).map(|(v, r)| (Value::Int(((v << (64 - 8 * n)) as i64) >> (64 - 8 * n)), r))
        }
        0xd9..=0xdb => take(1 << (t - 0xd9), rest).and_then(|(n, r)| string(n as usize, r)),
        // bin 8/16/32
        0xc4..=0xc6 => take(1 << (t - 0xc4), rest).and_then(|(n, r)| string(n as usize, r)),
        0xdc | 0xdd => take(2 << (t - 0xdc), rest).and_then(|(n, r)| array(n as usize, r)),
        0xde | 0xdf => take(2 << (t - 0xde), rest).and_then(|(n, r)| map(n as usize, r)),
        0xe0..=0xff => Some((Value::Int(t as i8 as i64), rest)),
        _ => None,
    }
}

fn encode(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Nil => out.push(0xc0),
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Int(i) => match *i {
            0..=0x7f => out.push(*i as u8),
            -32..=-1 => out.push(*i as i8 as u8),
            0x80..=0xffff => {
                out.push(0xcd);
                out.extend((*i as u16).to_be_bytes());
            }
            0x1_0000..=0xffff_ffff => {
                out.push(0xce);
                out.extend((*i as u32).to_be_bytes());
            }
            _ => {
                out.push(0xd3);
                out.extend(i.to_be_bytes());
            }
        },
        Value::Str(s) => raw(s.as_bytes(), out),
        Value::Bin(b) => raw(b, out),
        Value::Array(a) => {
            if a.len() < 16 {
                out.push(0x90 | a.len() as u8);
            } else {
                out.push(0xdc);
                out.extend((a.len() as u16).to_be_bytes());
            }
            a.iter().for_each(|x| encode(x, out));
        }
        Value::Map(m) => {
            if m.len() < 16 {
                out.push(0x80 | m.len() as u8);
            } else {
                out.push(0xde);
                out.extend((m.len() as u16).to_be_bytes());
            }
            for (k, x) in m {
                encode(k, out);
                encode(x, out);
            }
        }
    }
}

/// A raw (MessagePack's old format of libcts' 2015 msgpack-c: no str 8, no bin).
fn raw(b: &[u8], out: &mut Vec<u8>) {
    match b.len() {
        n @ 0..=31 => out.push(0xa0 | n as u8),
        n @ 32..=0xffff => {
            out.push(0xda);
            out.extend((n as u16).to_be_bytes());
        }
        n => {
            out.push(0xdb);
            out.extend((n as u32).to_be_bytes());
        }
    }
    out.extend(b);
}

/// AES-128-CBC with `iv`, PKCS#7.
fn encrypt(iv: &[u8], plain: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(&KEY));
    let pad = 16 - plain.len() % 16;
    let mut data = plain.to_vec();
    data.extend(std::iter::repeat_n(pad as u8, pad));
    let mut prev: [u8; 16] = iv.try_into().unwrap();
    for block in data.chunks_exact_mut(16) {
        block.iter_mut().zip(prev).for_each(|(b, p)| *b ^= p);
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
        prev.copy_from_slice(block);
    }
    data
}

fn decrypt(iv: &[u8], cipher_text: &[u8]) -> Option<Vec<u8>> {
    if cipher_text.is_empty() || cipher_text.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes128::new(GenericArray::from_slice(&KEY));
    let mut data = cipher_text.to_vec();
    let mut prev: [u8; 16] = iv.try_into().ok()?;
    for block in data.chunks_exact_mut(16) {
        let saved: [u8; 16] = (*block).try_into().unwrap();
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
        block.iter_mut().zip(prev).for_each(|(b, p)| *b ^= p);
        prev = saved;
    }
    let pad = *data.last()? as usize;
    (1..=16).contains(&pad).then(|| data.truncate(data.len() - pad))?;
    Some(data)
}

/// A request's message (`[[keychip id, game id, version, place id, time], body]`).
fn request(body: &[u8]) -> Option<Value> {
    let (iv, data) = (body.get(..16)?, body.get(16..)?);
    let packed = decrypt(iv, data)?;
    let plain = miniz_oxide::inflate::decompress_to_vec_zlib(&packed).ok()?;
    decode(&plain).map(|(v, _)| v)
}

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &b| (0..8).fold(crc ^ b as u32, |c, _| if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 }))
}

/// The answer's body: the header, the encrypted message.
fn answer(message: &Value) -> Vec<u8> {
    let mut plain = Vec::new();
    encode(message, &mut plain);
    let mut header = [0u8; 16];
    header[..2].copy_from_slice(&[2, 1]);
    header[4..8].copy_from_slice(&crc32(&plain).to_be_bytes());
    header[8..12].copy_from_slice(&(plain.len() as u32).to_be_bytes());
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6);
    [header.as_slice(), &encrypt(&header, &packed)].concat()
}

/// The value at `path` (indexes into nested arrays) of a request's body (`[header, body...]`:
/// index 1 is the body's first element).
fn at<'a>(message: &'a Option<Value>, path: &[usize]) -> Option<&'a Value> {
    let mut v = message.as_ref()?;
    for &i in path {
        v = match v {
            Value::Array(a) => a.get(i)?,
            _ => return None,
        };
    }
    Some(v)
}

fn message_at(message: &Option<Value>, path: &[usize]) -> Option<i64> {
    match at(message, path)? {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}

/// A save slot's size.
const SLOT: usize = 0x400;

/// An Aime card's player, kept in `Y:\cts\<cts id>.name` and `.bin` (the save slots).
struct Player {
    name: String,
    save: Vec<u8>,
}

impl Player {
    fn path(id: i64, ext: &str) -> String {
        format!("Y:\\cts\\{id}.{ext}")
    }

    fn load(id: i64) -> Player {
        Player { name: std::fs::read_to_string(Player::path(id, "name")).unwrap_or_default(), save: std::fs::read(Player::path(id, "bin")).unwrap_or_default() }
    }

    fn store(&self, id: i64) {
        let _ = std::fs::create_dir_all("Y:\\cts");
        let ok = std::fs::write(Player::path(id, "name"), &self.name).and(std::fs::write(Player::path(id, "bin"), &self.save));
        log!("cts: player {id} ({:?}) saved ({} bytes): {ok:?}", self.name, self.save.len());
    }
}

/// The request's body (`[header, body]`'s second element).
fn body(message: &Option<Value>) -> Option<&Vec<Value>> {
    match message {
        Some(Value::Array(m)) => match m.get(1) {
            Some(Value::Array(b)) => Some(b),
            _ => None,
        },
        _ => None,
    }
}

fn reply(api: &str, message: &Option<Value>) -> Value {
    match api {
        // one (empty) entry list per ranking id
        "GetRanking" => {
            let ids = body(message).and_then(|b| match b.get(1) {
                Some(Value::Array(ids)) => Some(ids.len()),
                _ => None,
            });
            Value::Array(vec![Value::Array(Vec::new()); ids.unwrap_or(0)])
        }
        // the player: [cts id, ""], [name, start slot, save data]: the card's saved player, else a
        // new one (no name: the game asks for it and resets the save)
        "StartGame" => {
            let id = message_at(message, &[1, 0]).unwrap_or(1);
            let (start, count) = (message_at(message, &[2, 0]).unwrap_or(0).clamp(0, 63) as usize, message_at(message, &[2, 1]).unwrap_or(2).clamp(1, 64) as usize);
            let player = Player::load(id);
            let save = player.save.get(start * SLOT..).unwrap_or(&[]).iter().copied().chain(std::iter::repeat(0)).take(count * SLOT).collect();
            log!("cts: player {id} ({:?}), slots {start}+{count}", player.name);
            Value::Array(vec![Value::Array(vec![Value::Int(id), Value::Str(String::new())]), Value::Array(vec![Value::Str(player.name.clone()), Value::Int(start as i64), Value::Bin(save)])])
        }
        // the player ([cts id, ""]), its name and the save slot that changed ([name, slot, data])
        "EndGame" | "ContinueGame" | "UpdateUser" => {
            if let Some(id) = message_at(message, &[1, 0]) {
                let mut player = Player::load(id);
                if let Some(Value::Str(name)) = at(message, &[2, 0])
                    && !name.is_empty()
                {
                    player.name = name.clone();
                }
                if let (Some(slot), Some(Value::Bin(data))) = (message_at(message, &[2, 1]), at(message, &[2, 2])) {
                    let at = slot.clamp(0, 63) as usize * SLOT;
                    if player.save.len() < at + data.len() {
                        player.save.resize(at + data.len(), 0);
                    }
                    player.save[at..at + data.len()].copy_from_slice(data);
                }
                player.store(id);
            }
            Value::Array(Vec::new())
        }
        _ => Value::Array(Vec::new()),
    }
}

/// Requests logged once each (GetRanking is polled while the game loads).
static LOGGED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn client(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let api = path.rsplit('/').next().unwrap_or("").to_string();
    let message = if body.is_empty() { None } else { request(&body) };
    let answer_message = reply(&api, &message);
    {
        let mut logged = LOGGED.lock().unwrap();
        if api != "GetRanking" || !logged.contains(&api) {
            logged.push(api.clone());
            log!("cts: {api} {message:?} -> {answer_message:?}");
        }
    }
    let out_body = answer(&answer_message);
    let mut out = stream;
    out.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out_body.len()).as_bytes())?;
    out.write_all(&out_body)
}

fn serve() {
    let port = crate::gs::port();
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            log!("cts: server on 127.0.0.1:{port}");
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let _ = client(stream);
                });
            }
        }
        Err(e) => log!("cts: 127.0.0.1:{port}: {e}"),
    }
}

pub(crate) fn init() {
    std::thread::spawn(serve);
}
