//! The title server's message format (SEGA's `libcts`): a 16-byte header, then the MessagePack
//! data zlib-compressed and AES-128-CBC encrypted (PKCS#7 padding), the header being the IV.
//!
//! * request header: `02 01 03 28`, the game id (4 characters), the data's CRC32 (big endian),
//!   4 random bytes;
//! * response header: `02 01`, a status (u16 big endian, 0 = success), the data's CRC32, its
//!   length (both big endian), 4 bytes.
//!
//! The data: positional MessagePack arrays, no keys. Requests: `[[keychip, game id, version,
//! 1, YYYYMMDDhhmmss], arguments...]`.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};

/// The AES key of Under Night In-Birth Exe:Late[cl-r]'s `libcts` (`BCryptGenerateSymmetricKey`).
const KEY: [u8; 16] = [0x8a, 0x8d, 0xf8, 0x0d, 0x93, 0xda, 0x05, 0x2a, 0x14, 0xd0, 0x49, 0x0e, 0xe2, 0x3a, 0x59, 0xd1];

// ---- MessagePack ----

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    /// A MessagePack str: the game's strings hold raw bytes too (the save data).
    Str(Vec<u8>),
    Bin(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

impl Value {
    pub(crate) fn int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn array(&self) -> &[Value] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }

    pub(crate) fn get(&self, i: usize) -> &Value {
        self.array().get(i).unwrap_or(&Value::Nil)
    }

    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Nil => out.push(0xc0),
            Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
            Value::Int(v) => {
                let v = *v;
                if (0..=0x7f).contains(&v) {
                    out.push(v as u8);
                } else if (-32..0).contains(&v) {
                    out.push(v as i8 as u8);
                } else if (0..=0xff).contains(&v) {
                    out.extend([0xcc, v as u8]);
                } else if (0..=0xffff).contains(&v) {
                    out.push(0xcd);
                    out.extend((v as u16).to_be_bytes());
                } else if (0..=0xffff_ffff).contains(&v) {
                    out.push(0xce);
                    out.extend((v as u32).to_be_bytes());
                } else if v >= 0 {
                    out.push(0xcf);
                    out.extend((v as u64).to_be_bytes());
                } else if v >= i32::MIN as i64 {
                    out.push(0xd2);
                    out.extend((v as i32).to_be_bytes());
                } else {
                    out.push(0xd3);
                    out.extend(v.to_be_bytes());
                }
            }
            Value::Str(s) => {
                let n = s.len();
                if n < 32 {
                    out.push(0xa0 | n as u8);
                } else if n < 0x100 {
                    out.extend([0xd9, n as u8]);
                } else if n < 0x10000 {
                    out.push(0xda);
                    out.extend((n as u16).to_be_bytes());
                } else {
                    out.push(0xdb);
                    out.extend((n as u32).to_be_bytes());
                }
                out.extend(s);
            }
            Value::Bin(b) => {
                let n = b.len();
                if n < 0x100 {
                    out.extend([0xc4, n as u8]);
                } else if n < 0x10000 {
                    out.push(0xc5);
                    out.extend((n as u16).to_be_bytes());
                } else {
                    out.push(0xc6);
                    out.extend((n as u32).to_be_bytes());
                }
                out.extend(b);
            }
            Value::Array(a) => {
                let n = a.len();
                if n < 16 {
                    out.push(0x90 | n as u8);
                } else if n < 0x10000 {
                    out.push(0xdc);
                    out.extend((n as u16).to_be_bytes());
                } else {
                    out.push(0xdd);
                    out.extend((n as u32).to_be_bytes());
                }
                for v in a {
                    v.write(out);
                }
            }
            Value::Map(m) => {
                let n = m.len();
                if n < 16 {
                    out.push(0x80 | n as u8);
                } else {
                    out.push(0xde);
                    out.extend((n as u16).to_be_bytes());
                }
                for (k, v) in m {
                    k.write(out);
                    v.write(out);
                }
            }
        }
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    /// One value from `data` at `*pos`.
    pub(crate) fn read(data: &[u8], pos: &mut usize) -> Option<Value> {
        let take = |pos: &mut usize, n: usize| -> Option<&[u8]> {
            let s = data.get(*pos..*pos + n)?;
            *pos += n;
            Some(s)
        };
        let be = |b: &[u8]| b.iter().fold(0u64, |a, &x| a << 8 | x as u64);
        let t = *take(pos, 1)?.first()?;
        let len = |pos: &mut usize, n: usize| -> Option<usize> { Some(be(take(pos, n)?) as usize) };
        let seq = |pos: &mut usize, n: usize| -> Option<Vec<Value>> { (0..n).map(|_| Value::read(data, pos)).collect() };
        let map = |pos: &mut usize, n: usize| -> Option<Vec<(Value, Value)>> {
            (0..n).map(|_| Some((Value::read(data, pos)?, Value::read(data, pos)?))).collect()
        };
        let text = |b: &[u8]| Value::Str(b.to_vec());
        Some(match t {
            0x00..=0x7f => Value::Int(t as i64),
            0x80..=0x8f => Value::Map(map(pos, (t & 0xf) as usize)?),
            0x90..=0x9f => Value::Array(seq(pos, (t & 0xf) as usize)?),
            0xa0..=0xbf => text(take(pos, (t & 0x1f) as usize)?),
            0xc0 => Value::Nil,
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            0xc4 | 0xc5 | 0xc6 => {
                let n = len(pos, 1 << (t - 0xc4))?;
                Value::Bin(take(pos, n)?.to_vec())
            }
            0xca => Value::Int(f32::from_be_bytes(take(pos, 4)?.try_into().ok()?) as i64),
            0xcb => Value::Int(f64::from_be_bytes(take(pos, 8)?.try_into().ok()?) as i64),
            0xcc..=0xcf => Value::Int(be(take(pos, 1 << (t - 0xcc))?) as i64),
            0xd0 => Value::Int(take(pos, 1)?[0] as i8 as i64),
            0xd1 => Value::Int(i16::from_be_bytes(take(pos, 2)?.try_into().ok()?) as i64),
            0xd2 => Value::Int(i32::from_be_bytes(take(pos, 4)?.try_into().ok()?) as i64),
            0xd3 => Value::Int(i64::from_be_bytes(take(pos, 8)?.try_into().ok()?)),
            0xd9 | 0xda | 0xdb => {
                let n = len(pos, 1 << (t - 0xd9))?;
                text(take(pos, n)?)
            }
            0xdc | 0xdd => {
                let n = len(pos, if t == 0xdc { 2 } else { 4 })?;
                Value::Array(seq(pos, n)?)
            }
            0xde | 0xdf => {
                let n = len(pos, if t == 0xde { 2 } else { 4 })?;
                Value::Map(map(pos, n)?)
            }
            0xe0..=0xff => Value::Int(t as i8 as i64),
            _ => return None,
        })
    }
}

/// Every value of `data` (the requests hold one).
pub(crate) fn decode_all(data: &[u8]) -> Vec<Value> {
    let mut pos = 0;
    let mut values = Vec::new();
    while pos < data.len() {
        match Value::read(data, &mut pos) {
            Some(v) => values.push(v),
            None => break,
        }
    }
    values
}

// ---- the envelope ----

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { crc >> 1 ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn cbc_encrypt(iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(&KEY));
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

fn cbc_decrypt(iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes128::new(GenericArray::from_slice(&KEY));
    let mut prev: [u8; 16] = iv.try_into().ok()?;
    let mut out = Vec::with_capacity(data.len());
    for block in data.chunks(16) {
        let mut ga = GenericArray::clone_from_slice(block);
        cipher.decrypt_block(&mut ga);
        for i in 0..16 {
            out.push(ga[i] ^ prev[i]);
        }
        prev.copy_from_slice(block);
    }
    let pad = *out.last()? as usize;
    if pad == 0 || pad > 16 || pad > out.len() {
        return None;
    }
    out.truncate(out.len() - pad);
    Some(out)
}

/// A request body: its MessagePack data, decompressed.
pub(crate) fn open_request(body: &[u8]) -> Option<Vec<u8>> {
    if body.len() < 32 {
        return None;
    }
    let compressed = cbc_decrypt(&body[..16], &body[16..])?;
    miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).ok()
}

/// A response body: `status` and the MessagePack `data` (none: the header alone).
pub(crate) fn seal_response(status: u16, data: Option<&[u8]>) -> Vec<u8> {
    let raw = data.unwrap_or(&[]);
    let mut header = [0u8; 16];
    header[0] = 0x02;
    header[1] = 0x01;
    header[2..4].copy_from_slice(&status.to_be_bytes());
    header[4..8].copy_from_slice(&crc32(raw).to_be_bytes());
    header[8..12].copy_from_slice(&(raw.len() as u32).to_be_bytes());
    // the IV's last bytes: any
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    header[12..16].copy_from_slice(&seed.to_le_bytes());
    let mut out = header.to_vec();
    if data.is_some() {
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(raw, 6);
        out.extend(cbc_encrypt(&header, &compressed));
    }
    out
}

/// `[a, b]`-style helpers.
pub(crate) fn arr(items: Vec<Value>) -> Value {
    Value::Array(items)
}

pub(crate) fn int(v: i64) -> Value {
    Value::Int(v)
}

pub(crate) fn s(v: &str) -> Value {
    Value::Str(v.as_bytes().to_vec())
}

/// A short readable form for the log.
pub(crate) fn show(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Str(s) if s.len() > 64 => format!("str({} bytes)", s.len()),
        Value::Str(s) => format!("{:?}", String::from_utf8_lossy(s)),
        Value::Bin(b) => format!("bin({} bytes)", b.len()),
        Value::Array(a) => format!("[{}]", a.iter().map(show).collect::<Vec<_>>().join(", ")),
        Value::Map(m) => format!("{{{}}}", m.iter().map(|(k, v)| format!("{}: {}", show(k), show(v))).collect::<Vec<_>>().join(", ")),
    }
}
