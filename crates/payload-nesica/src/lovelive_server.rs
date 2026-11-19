//! Love Live! School Idol Festival ~after school ACTIVITY~ Next Stage's title server (Square
//! Enix's, its host from the NESYS certificate), the payload's own on `127.0.0.1:37704`: plain
//! HTTP, after LLServer (BocuD, MIT).
//!
//! * `POST /basicinfo/`: the game's first request, its answer the servers and the session
//!   cipher, `{"result":200,"response":{"base_url","download_url","key","iv","tenpo_index"}}`,
//!   encrypted with an RSA private key (PKCS#1 v1.5, block type 1): the game decrypts it with
//!   the public key in its executable (a PEM text, +0x596600). That key is LLServer's here:
//!   [`patch_key`] writes its public PEM over the game's (the same length) at start.
//! * `POST /game`: every game request, AES-128-CFB (key: the first 16 characters of `key`, IV
//!   `iv`, PKCS#7 padding) both ways; inside, JSON `{"protocol","param","sessionkey","terminal"}`
//!   answered `{"result","response"}` by `lovelive_player.rs`.
//! * The NESYS service's alive checks and the other pages: answered empty.
//!
//! `WAL_LOVELIVE_SERVER_LOG=1`: every request and answer in the log.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;

use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::json::Json;
use wal_payload_common::log;

pub(crate) const PORT: u16 = 37704;
/// The session cipher's key (its first 16 characters) and IV, LLServer's.
const KEY: &str = "01234567890123456789012345678901";
const IV: &str = "0123456789012345";
/// The shop's index (the NESYS certificate's).
const TENPO_INDEX: u32 = 1337;

/// LLServer's RSA key: modulus and private exponent (hex, big endian), public key (PEM).
const MODULUS: &str = concat!(
    "cbade7c9b0e0d9dd25e449b9453b31d102785a1b93e03c2bcc90ff49d3c36cea2d5c4e41895c9c7cdc5c7d75526bbe12",
    "bea4e1c90b3829ab19c8afcd58937a5f28bb350821db12987377958fc6fc312921cf963beb7d5db1c2d0451f6c0e24df",
    "dbe8d1f2e99de94f68a7f66e71a394cda11cc876a5af279e4700fcabb9ad94171c2fee5d495556b8f689fce8783a2be4",
    "e2a36e9d81f2810fe2c36bc180dddfead075ca22a551ed13e75152d7225c6af5a2969d8903a5c61218743a61495fac1f",
    "e6f3eef617935c6aa7ae20a58a75e75d5d7350f0da6b4b9d0fc9f6395f4da61a33a83ed34f8a44ebc1bae57cff88b01a",
    "12e75283b7f54a1bbe545b6dfdc17837",
);
const PRIVATE_EXPONENT: &str = concat!(
    "11d48c5d2bdbfb3bdb5f293f0bfafb1e4774e965244f0b25706d39bea8f2f63167e8293cf3d29a2bbf64ba0ce5100e88",
    "788d044103ccebaf9151c56618bccc5d268fd63c25d940e9a74e0ed73d2858560d4cd80e3076d3417102fcd21bdfa9c5",
    "b916cac3aa44e07c4af6a28c42417b90d16baccc34d93e5822477401c529975ee881cee0b47ee7f1a660a8c72b105cb0",
    "98733ecef2b045eab995dc0a340af24e6a1db300d5b10120e5c5a2af2b559c6bdf9fb766973976359fe033cd136bb1bd",
    "49db968d9aa73fefc1d390bac96616c1154a5729dc9a12b844689ec135b8ebc7d244433e0b0efa44d051634a6794c426",
    "835a3e9bbef3d25106c4510247988edd",
);
const PUBLIC_PEM: &str = concat!(
    "-----BEGIN PUBLIC KEY-----\n",
    "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAy63nybDg2d0l5Em5RTsx\n",
    "0QJ4WhuT4DwrzJD/SdPDbOotXE5BiVycfNxcfXVSa74SvqThyQs4KasZyK/NWJN6\n",
    "Xyi7NQgh2xKYc3eVj8b8MSkhz5Y7631dscLQRR9sDiTf2+jR8umd6U9op/ZucaOU\n",
    "zaEcyHalryeeRwD8q7mtlBccL+5dSVVWuPaJ/Oh4Oivk4qNunYHygQ/iw2vBgN3f\n",
    "6tB1yiKlUe0T51FS1yJcavWilp2JA6XGEhh0OmFJX6wf5vPu9heTXGqnriClinXn\n",
    "XV1zUPDaa0udD8n2OV9NphozqD7TT4pE68G65Xz/iLAaEudSg7f1Shu+VFtt/cF4\n",
    "NwIDAQAB\n",
    "-----END PUBLIC KEY-----\n",
);
/// The game's public key (2.4.1.000): where, and how it starts after the PEM header.
const GAME_PEM: usize = 0x59_6600;
const GAME_PEM_START: &[u8] = b"-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA21J2pR7c";

fn verbose() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("WAL_LOVELIVE_SERVER_LOG").is_ok_and(|v| v.trim() == "1"))
}

/// The server's base address (the NESYS certificate's game server).
pub(crate) fn url() -> String {
    format!("http://127.0.0.1:{PORT}")
}

/// Starts the server (once).
pub(crate) fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        patch_key();
        match TcpListener::bind(("127.0.0.1", PORT)) {
            Ok(listener) => {
                log!("lovelive: title server on 127.0.0.1:{PORT}");
                std::thread::spawn(move || {
                    // the basicinfo answer (constant), computed before the game asks
                    basic_info();
                    for stream in listener.incoming().flatten() {
                        std::thread::spawn(move || {
                            if let Err(e) = serve(stream) {
                                log!("lovelive: title server: {e}");
                            }
                        });
                    }
                });
            }
            Err(e) => log!("lovelive: title server: 127.0.0.1:{PORT}: {e}"),
        }
    });
}

/// The game's RSA public key replaced by the server's.
fn patch_key() {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let at = (exe + GAME_PEM) as *mut u8;
    let found = unsafe { std::slice::from_raw_parts(at, PUBLIC_PEM.len() + 1) };
    if !found.starts_with(GAME_PEM_START) || found[PUBLIC_PEM.len() - 1] != b'\n' || found[PUBLIC_PEM.len()] != 0 {
        log!("lovelive: unknown executable: its public key not replaced (no title server)");
        return;
    }
    let hex: String = PUBLIC_PEM.bytes().map(|b| format!("{b:02X}")).collect();
    wal_payload_common::patches::apply_list(&format!("{GAME_PEM:x}:{hex}"));
    log!("lovelive: the game's public key replaced");
}

/// One request, answered; the connection closed.
fn serve(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut length, mut expect) = (0usize, false);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or((&header, ""));
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "expect" => expect = value.trim().eq_ignore_ascii_case("100-continue"),
            _ => {}
        }
    }
    let mut out = stream;
    if expect {
        out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let path = target.split('?').next().unwrap_or("").trim_matches('/').to_ascii_lowercase();
    let (kind, reply) = match path.as_str() {
        "basicinfo" => {
            log!("lovelive: basicinfo");
            ("application/octet-stream", basic_info().to_vec())
        }
        "game" => ("application/octet-stream", game(&body)),
        _ => {
            log!("lovelive: {method} {target} ({} bytes): answered empty", body.len());
            ("text/plain", Vec::new())
        }
    };
    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len());
    out.write_all(head.as_bytes())?;
    out.write_all(&reply)?;
    out.flush()
}

/// A `/game` request: decrypted, answered, encrypted.
fn game(body: &[u8]) -> Vec<u8> {
    let plain = cfb(body, false);
    let plain = unpad(&plain);
    let text: String = String::from_utf8_lossy(plain).chars().filter(|&c| c != '\0').collect();
    let reply = match Json::parse(&text) {
        Some(request) => {
            let reply = crate::lovelive_player::handle(&request).to_string();
            if verbose() {
                log!("lovelive: {text} -> {reply}");
            } else {
                log!("lovelive: {} ({} bytes answered)", request.s("protocol"), reply.len());
            }
            reply
        }
        None => {
            log!("lovelive: /game: not JSON ({} bytes)", body.len());
            Json::obj().with("result", 400).with("response", Json::obj()).to_string()
        }
    };
    cfb(&pad(reply.as_bytes()), true)
}

// ── the session cipher ───────────────────────────────────────────────────────────────────

/// AES-128-CFB (128-bit feedback) with the session key and IV.
fn cfb(data: &[u8], encrypt: bool) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(&KEY.as_bytes()[..16]));
    let mut feedback = [0u8; 16];
    feedback.copy_from_slice(IV.as_bytes());
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut stream = GenericArray::clone_from_slice(&feedback);
        cipher.encrypt_block(&mut stream);
        let start = out.len();
        out.extend(chunk.iter().zip(stream.iter()).map(|(d, k)| d ^ k));
        let cipher_text = if encrypt { &out[start..] } else { chunk };
        feedback[..cipher_text.len()].copy_from_slice(cipher_text);
    }
    out
}

fn pad(data: &[u8]) -> Vec<u8> {
    let n = 16 - data.len() % 16;
    let mut out = data.to_vec();
    out.extend(std::iter::repeat_n(n as u8, n));
    out
}

/// PKCS#7 padding removed (the data kept whole when it has none).
fn unpad(data: &[u8]) -> &[u8] {
    match data.last() {
        Some(&n) if (1..=16).contains(&n) && data.len() >= n as usize && data[data.len() - n as usize..].iter().all(|&b| b == n) => {
            &data[..data.len() - n as usize]
        }
        _ => data,
    }
}

// ── basicinfo: RSA ───────────────────────────────────────────────────────────────────────

/// The basicinfo answer: its JSON in one RSA block (PKCS#1 v1.5 type 1, private key).
fn basic_info() -> &'static [u8] {
    static ANSWER: OnceLock<Vec<u8>> = OnceLock::new();
    ANSWER.get_or_init(|| {
        let base = url();
        let response = Json::obj()
            .with("base_url", format!("{base}/game"))
            .with("download_url", format!("{base}/download"))
            .with("key", KEY)
            .with("iv", IV)
            .with("tenpo_index", TENPO_INDEX);
        let json = Json::obj().with("result", 200).with("response", response).to_string();
        let n = limbs(MODULUS);
        let size = MODULUS.len() / 2;
        // 00 01 FF..FF 00 data
        let mut block = vec![0xFFu8; size];
        block[0] = 0;
        block[1] = 1;
        let data = json.as_bytes();
        block[size - data.len() - 1] = 0;
        block[size - data.len()..].copy_from_slice(data);
        let m: Vec<u32> = block.rchunks(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]])).collect();
        let c = mod_pow(&m, &limbs(PRIVATE_EXPONENT), &n);
        c.iter().rev().flat_map(|l| l.to_be_bytes()).collect()
    })
}

/// A hex number as 32-bit limbs, lowest first.
fn limbs(hex: &str) -> Vec<u32> {
    hex.as_bytes().rchunks(8).map(|c| u32::from_str_radix(std::str::from_utf8(c).unwrap_or("0"), 16).unwrap_or(0)).collect()
}

/// Montgomery product `a * b / R mod n` (R = 2^(32 * limbs)), `inv` = `-1 / n[0] mod 2^32`.
fn mont_mul(a: &[u32], b: &[u32], n: &[u32], inv: u32) -> Vec<u32> {
    let len = n.len();
    let mut t = vec![0u32; len + 2];
    for &ai in a {
        let mut carry = 0u64;
        for j in 0..len {
            let v = t[j] as u64 + ai as u64 * b[j] as u64 + carry;
            t[j] = v as u32;
            carry = v >> 32;
        }
        let v = t[len] as u64 + carry;
        t[len] = v as u32;
        t[len + 1] = (v >> 32) as u32;
        let m = t[0].wrapping_mul(inv);
        let mut carry = (t[0] as u64 + m as u64 * n[0] as u64) >> 32;
        for j in 1..len {
            let v = t[j] as u64 + m as u64 * n[j] as u64 + carry;
            t[j - 1] = v as u32;
            carry = v >> 32;
        }
        let v = t[len] as u64 + carry;
        t[len - 1] = v as u32;
        t[len] = t[len + 1] + (v >> 32) as u32;
    }
    // t >= n: t -= n
    let ge = t[len] != 0 || (0..len).rev().find_map(|i| (t[i] != n[i]).then_some(t[i] > n[i])).unwrap_or(true);
    if ge {
        let mut borrow = 0i64;
        for (i, l) in t.iter_mut().enumerate().take(len) {
            let v = *l as i64 - n[i] as i64 - borrow;
            borrow = i64::from(v < 0);
            *l = v.rem_euclid(1 << 32) as u32;
        }
    }
    t.truncate(len);
    t
}

/// `base^exp mod n` (n odd).
fn mod_pow(base: &[u32], exp: &[u32], n: &[u32]) -> Vec<u32> {
    let len = n.len();
    // -1 / n[0] mod 2^32 (Newton)
    let mut inv = 1u32;
    for _ in 0..5 {
        inv = inv.wrapping_mul(2u32.wrapping_sub(n[0].wrapping_mul(inv)));
    }
    let inv = inv.wrapping_neg();
    // R^2 mod n: 1 doubled 2 * 32 * len times, reduced each time
    let mut r2 = vec![0u32; len];
    r2[0] = 1;
    for _ in 0..64 * len {
        let mut carry = 0;
        for l in r2.iter_mut() {
            let next = *l >> 31;
            *l = *l << 1 | carry;
            carry = next;
        }
        let ge = carry != 0 || (0..len).rev().find_map(|i| (r2[i] != n[i]).then_some(r2[i] > n[i])).unwrap_or(true);
        if ge {
            let mut borrow = 0i64;
            for (i, l) in r2.iter_mut().enumerate() {
                let v = *l as i64 - n[i] as i64 - borrow;
                borrow = i64::from(v < 0);
                *l = v.rem_euclid(1 << 32) as u32;
            }
        }
    }
    let mut padded = base.to_vec();
    padded.resize(len, 0);
    let x = mont_mul(&padded, &r2, n, inv);
    let mut one = vec![0u32; len];
    one[0] = 1;
    let mut acc = mont_mul(&one, &r2, n, inv);
    for bit in (0..exp.len() * 32).rev() {
        acc = mont_mul(&acc, &acc, n, inv);
        if exp[bit / 32] >> (bit % 32) & 1 == 1 {
            acc = mont_mul(&acc, &x, n, inv);
        }
    }
    mont_mul(&acc, &one, n, inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The private operation undone by the public exponent (65537).
    #[test]
    fn rsa_round_trip() {
        let n = limbs(MODULUS);
        let c = basic_info();
        let c: Vec<u32> = c.rchunks(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]])).collect();
        let m = mod_pow(&c, &[65537], &n);
        let bytes: Vec<u8> = m.iter().rev().flat_map(|l| l.to_be_bytes()).collect();
        assert_eq!(&bytes[..2], &[0, 1]);
        assert!(String::from_utf8_lossy(&bytes).contains("\"base_url\""));
    }

    #[test]
    fn cfb_round_trip() {
        let data = b"{\"protocol\":\"auth\"} and more than one block";
        assert_eq!(cfb(&cfb(data, true), false), data);
    }
}
