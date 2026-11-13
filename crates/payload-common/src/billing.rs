//! ALL.Net's billing server (`ib.naominet.jp:8443`), reached by a game whose amLib checks in
//! itself (CHUNITHM; on ALLS, amdaemon): its socket to the router's port 8443 goes to this
//! server, 127.0.0.1:`WAL_PORT`+105 (`allnet.rs`). The answers follow ARTEMiS's
//! `core/allnet.py` billing servlet (WTFPL).
//!
//! TLS: the game's OpenSSL 1.0.2 offers RSA key exchange only (`TLS_RSA_WITH_AES_*_CBC_SHA`,
//! which TLS libraries no longer serve): a TLS server of these suites alone (1.2, and 1.0 / 1.1
//! for the clients offering no more: CHUNITHM PLUS's OpenSSL), with
//! segatools' dummy PKI (public domain): the certificate `ib.naominet.jp`, signed by the CA the
//! keychip gives the game (`pki.rs`), and its RSA key.
//!
//! A request: a POST of raw-deflated lines (`application/octet-stream`), each a form; the first
//! the cabinet's (`keychipid`, `playcnt`, `playlimit`, `nearfull`, `billingtype`, `requestno`,
//! `traceleft`...), the others its trace logs. Trace logs left: asked first (`result=6`); then
//! the play limit (raised by 1024 plays when reached) and the near-full count, each signed with
//! the billing key whose public half the keychip gives the game (`pki::BILLING_PUB`):
//! RSA PKCS#1 v1.5 over SHA-1 of the value (u32, little endian) and the keychip id.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use sha2::{Digest, Sha256};
use wal_payload_common::log;

pub(crate) fn port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 105
}

// ── RSA: Montgomery multiplication, 32-bit limbs from the lowest ──────────────────────────

fn limbs(bytes: &[u8], n: usize) -> Vec<u32> {
    let mut out = vec![0u32; n];
    for (i, &b) in bytes.iter().rev().enumerate() {
        out[i / 4] |= (b as u32) << (8 * (i % 4));
    }
    out
}

fn ge(a: &[u32], b: &[u32]) -> bool {
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
    }
    true
}

fn sub_in_place(a: &mut [u32], b: &[u32]) {
    let mut borrow = 0u64;
    for i in 0..a.len() {
        let v = (a[i] as u64).wrapping_sub(b[i] as u64).wrapping_sub(borrow);
        borrow = (v >> 63) & 1;
        a[i] = v as u32;
    }
}

/// `a * b / R mod n` (CIOS; `inv` = -n⁻¹ mod 2³²).
fn mont_mul(a: &[u32], b: &[u32], n: &[u32], inv: u32) -> Vec<u32> {
    let s = n.len();
    let mut t = vec![0u32; s + 2];
    for &ai in a {
        let mut carry = 0u64;
        for j in 0..s {
            let v = t[j] as u64 + ai as u64 * b[j] as u64 + carry;
            t[j] = v as u32;
            carry = v >> 32;
        }
        let v = t[s] as u64 + carry;
        t[s] = v as u32;
        t[s + 1] = (v >> 32) as u32;
        let m = t[0].wrapping_mul(inv);
        let mut carry = (t[0] as u64 + m as u64 * n[0] as u64) >> 32;
        for j in 1..s {
            let v = t[j] as u64 + m as u64 * n[j] as u64 + carry;
            t[j - 1] = v as u32;
            carry = v >> 32;
        }
        let v = t[s] as u64 + carry;
        t[s - 1] = v as u32;
        t[s] = t[s + 1] + (v >> 32) as u32;
    }
    let mut r = t[..=s].to_vec();
    let mut n1 = n.to_vec();
    n1.push(0);
    if ge(&r, &n1) {
        sub_in_place(&mut r, &n1);
    }
    r.truncate(s);
    r
}

/// `m^d mod n` (the private key operation), big endian in and out, `n`'s size.
fn rsa_private(m: &[u8], n: &[u8], d: &[u8]) -> Vec<u8> {
    let s = n.len().div_ceil(4);
    let (n, d, m) = (limbs(n, s), limbs(d, s), limbs(m, s));
    // -n⁻¹ mod 2³² (Newton's iteration)
    let mut x = 1u32;
    for _ in 0..5 {
        x = x.wrapping_mul(2u32.wrapping_sub(n[0].wrapping_mul(x)));
    }
    let inv = x.wrapping_neg();
    // R² mod n, by doublings of 1
    let mut r2 = vec![0u32; s + 1];
    r2[0] = 1;
    let mut n1 = n.clone();
    n1.push(0);
    for _ in 0..64 * s {
        let mut carry = 0;
        for l in r2.iter_mut() {
            let next = *l >> 31;
            *l = *l << 1 | carry;
            carry = next;
        }
        if ge(&r2, &n1) {
            sub_in_place(&mut r2, &n1);
        }
    }
    r2.truncate(s);
    let base = mont_mul(&m, &r2, &n, inv);
    let mut one = vec![0u32; s];
    one[0] = 1;
    let mut acc = mont_mul(&one, &r2, &n, inv);
    for i in (0..32 * s).rev() {
        acc = mont_mul(&acc, &acc, &n, inv);
        if d[i / 32] >> (i % 32) & 1 == 1 {
            acc = mont_mul(&acc, &base, &n, inv);
        }
    }
    let out = mont_mul(&acc, &one, &n, inv);
    out.iter().rev().flat_map(|l| l.to_be_bytes()).collect()
}

/// The billing key's signature of `data` (RSA PKCS#1 v1.5, SHA-1), hex.
fn sign(data: &[u8]) -> String {
    let size = BILLING_N.len();
    let digest = sha1_smol::Sha1::from(data).digest().bytes();
    // 00 01 FF.. 00, SHA-1's DigestInfo, the digest
    let info = [&[0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14][..], &digest].concat();
    let mut em = vec![0xFFu8; size];
    em[0] = 0;
    em[1] = 1;
    em[size - info.len() - 1] = 0;
    em[size - info.len()..].copy_from_slice(&info);
    rsa_private(&em, BILLING_N, BILLING_D).iter().map(|b| format!("{b:02x}")).collect()
}

// ── TLS 1.0-1.2, RSA key exchange, AES-CBC with HMAC-SHA1 ──────────────────────────────────

fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

fn sha1(data: &[u8]) -> Vec<u8> {
    sha1_smol::Sha1::from(data).digest().bytes().to_vec()
}

fn md5_digest(data: &[u8]) -> [u8; 16] {
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

fn md5(data: &[u8]) -> Vec<u8> {
    md5_digest(data).to_vec()
}

fn hmac(hash: fn(&[u8]) -> Vec<u8>, key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = if key.len() > 64 { hash(key) } else { key.to_vec() };
    k.resize(64, 0);
    let inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).chain(msg.iter().copied()).collect();
    let outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).chain(hash(&inner)).collect();
    hash(&outer)
}

/// P_hash: `len` bytes of HMAC expansion.
fn p_hash(hash: fn(&[u8]) -> Vec<u8>, secret: &[u8], seed: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut a = hmac(hash, secret, seed);
    while out.len() < len {
        out.extend(hmac(hash, secret, &[&a[..], seed].concat()));
        a = hmac(hash, secret, &a);
    }
    out.truncate(len);
    out
}

/// The PRF of TLS 1.`minor - 1`: 1.2's P_SHA256; 1.0's and 1.1's P_MD5 over the secret's first
/// half xored with P_SHA1 over its second half.
fn prf(minor: u8, secret: &[u8], label: &str, seed: &[u8], len: usize) -> Vec<u8> {
    let seed = [label.as_bytes(), seed].concat();
    if minor >= 3 {
        return p_hash(sha256, secret, &seed, len);
    }
    let half = secret.len().div_ceil(2);
    let (a, b) = (p_hash(md5, &secret[..half], &seed, len), p_hash(sha1, &secret[secret.len() - half..], &seed, len));
    a.iter().zip(&b).map(|(x, y)| x ^ y).collect()
}

/// The handshake's hash for the finished messages: 1.2's SHA-256, else MD5 and SHA-1.
fn transcript_hash(minor: u8, transcript: &[u8]) -> Vec<u8> {
    if minor >= 3 { sha256(transcript) } else { [md5(transcript), sha1(transcript)].concat() }
}

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(alg: usize, buf: *mut u8, len: u32, flags: u32) -> i32;
}

fn random(len: usize) -> Vec<u8> {
    let mut b = vec![0u8; len];
    // BCRYPT_USE_SYSTEM_PREFERRED_RNG
    unsafe { BCryptGenRandom(0, b.as_mut_ptr(), len as u32, 2) };
    b
}

enum Aes {
    K128(aes::Aes128),
    K256(aes::Aes256),
}

impl Aes {
    fn new(key: &[u8]) -> Aes {
        if key.len() == 16 { Aes::K128(aes::Aes128::new(GenericArray::from_slice(key))) } else { Aes::K256(aes::Aes256::new(GenericArray::from_slice(key))) }
    }

    fn encrypt(&self, block: &mut [u8]) {
        let b = GenericArray::from_mut_slice(block);
        match self {
            Aes::K128(c) => c.encrypt_block(b),
            Aes::K256(c) => c.encrypt_block(b),
        }
    }

    fn decrypt(&self, block: &mut [u8]) {
        let b = GenericArray::from_mut_slice(block);
        match self {
            Aes::K128(c) => c.decrypt_block(b),
            Aes::K256(c) => c.decrypt_block(b),
        }
    }
}

/// One direction's record protection.
struct Cipher {
    aes: Aes,
    mac_key: Vec<u8>,
    seq: u64,
    minor: u8,
    /// TLS 1.0's CBC chain: the key block's IV, then each record's last cipher block (1.1 and
    /// 1.2: an explicit IV in each record).
    chain: Vec<u8>,
}

impl Cipher {
    fn mac(&self, kind: u8, data: &[u8]) -> Vec<u8> {
        let header = [&self.seq.to_be_bytes()[..], &[kind, 3, self.minor], &(data.len() as u16).to_be_bytes()].concat();
        hmac(sha1, &self.mac_key, &[&header[..], data].concat())
    }

    /// A record's fragment: explicit IV (1.1, 1.2), then data, MAC and padding encrypted (CBC).
    fn seal(&mut self, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut plain = [data, &self.mac(kind, data)].concat();
        let pad = 15 - plain.len() % 16;
        plain.extend(std::iter::repeat_n(pad as u8, pad + 1));
        let iv = if self.minor >= 2 { random(16) } else { self.chain.clone() };
        let mut prev = iv.clone();
        for block in plain.chunks_mut(16) {
            block.iter_mut().zip(&prev).for_each(|(b, p)| *b ^= p);
            self.aes.encrypt(block);
            prev = block.to_vec();
        }
        self.seq += 1;
        self.chain = prev;
        if self.minor >= 2 { [iv, plain].concat() } else { plain }
    }

    fn open(&mut self, kind: u8, fragment: &[u8]) -> Result<Vec<u8>, String> {
        let iv_len = if self.minor >= 2 { 16 } else { 0 };
        if fragment.len() < iv_len + 32 || fragment.len() % 16 != 0 {
            return Err("bad record size".into());
        }
        let mut prev = if iv_len > 0 { fragment[..16].to_vec() } else { self.chain.clone() };
        self.chain = fragment[fragment.len() - 16..].to_vec();
        let mut plain = fragment[iv_len..].to_vec();
        for block in plain.chunks_mut(16) {
            let saved = block.to_vec();
            self.aes.decrypt(block);
            block.iter_mut().zip(&prev).for_each(|(b, p)| *b ^= p);
            prev = saved;
        }
        let pad = *plain.last().unwrap() as usize + 1;
        let end = plain.len().checked_sub(pad + 20).ok_or("bad padding")?;
        let (data, mac) = (plain[..end].to_vec(), plain[end..end + 20].to_vec());
        if self.mac(kind, &data) != mac {
            return Err("bad record MAC".into());
        }
        self.seq += 1;
        Ok(data)
    }
}

struct Tls {
    tcp: TcpStream,
    /// The protocol's minor version (3: TLS 1.2), the client's highest up to 1.2.
    minor: u8,
    read: Option<Cipher>,
    write: Option<Cipher>,
    /// Handshake messages read and not consumed.
    pending: Vec<u8>,
    /// The application data read and not consumed.
    data: Vec<u8>,
}

impl Tls {
    fn record(&mut self) -> Result<(u8, Vec<u8>), String> {
        let mut header = [0u8; 5];
        self.tcp.read_exact(&mut header).map_err(|e| e.to_string())?;
        let mut fragment = vec![0u8; u16::from_be_bytes([header[3], header[4]]) as usize];
        self.tcp.read_exact(&mut fragment).map_err(|e| e.to_string())?;
        let data = match &mut self.read {
            Some(c) => c.open(header[0], &fragment)?,
            None => fragment,
        };
        if header[0] == 21 {
            return Err(format!("alert {data:02x?}"));
        }
        Ok((header[0], data))
    }

    fn send(&mut self, kind: u8, data: &[u8]) -> Result<(), String> {
        for chunk in data.chunks(16384) {
            let fragment = match &mut self.write {
                Some(c) => c.seal(kind, chunk),
                None => chunk.to_vec(),
            };
            let record = [&[kind, 3, self.minor][..], &(fragment.len() as u16).to_be_bytes(), &fragment].concat();
            self.tcp.write_all(&record).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// The next handshake message (type, whole message).
    fn handshake(&mut self) -> Result<(u8, Vec<u8>), String> {
        loop {
            if self.pending.len() >= 4 {
                let len = u32::from_be_bytes([0, self.pending[1], self.pending[2], self.pending[3]]) as usize;
                if self.pending.len() >= 4 + len {
                    let msg: Vec<u8> = self.pending.drain(..4 + len).collect();
                    return Ok((msg[0], msg));
                }
            }
            match self.record()? {
                (22, data) => self.pending.extend(data),
                (kind, _) => return Err(format!("record {kind} in the handshake")),
            }
        }
    }

    /// The handshake: the session's keys.
    fn accept(&mut self) -> Result<(), String> {
        let mut transcript = Vec::new();
        let (kind, hello) = self.handshake()?;
        if kind != 1 || hello.len() < 43 {
            return Err("no client hello".into());
        }
        transcript.extend(&hello);
        let client_random = hello[6..38].to_vec();
        let mut at = 38 + 1 + hello[38] as usize;
        let count = u16::from_be_bytes([hello[at], hello[at + 1]]) as usize / 2;
        let suites: Vec<u16> = (0..count).map(|i| u16::from_be_bytes([hello[at + 2 + 2 * i], hello[at + 3 + 2 * i]])).collect();
        at += 2 + 2 * count;
        let _ = at;
        // TLS_RSA_WITH_AES_128_CBC_SHA, TLS_RSA_WITH_AES_256_CBC_SHA
        if std::env::var("WAL_CHUNI_DEBUG").is_ok() {
            log!("billing: client hello suites {suites:04x?}");
        }
        let suite = [0x002Fu16, 0x0035].into_iter().find(|s| suites.contains(s)).ok_or_else(|| format!("no RSA AES-CBC-SHA suite in {suites:04x?}"))?;
        let key_len = if suite == 0x002F { 16 } else { 32 };
        if hello[4] != 3 || hello[5] < 1 {
            return Err(format!("client version {:02x}{:02x}", hello[4], hello[5]));
        }
        self.minor = hello[5].min(3);
        let minor = self.minor;
        log!("billing: TLS 1.{}, suite {suite:04x}", minor - 1);
        let server_random = random(32);
        // server hello (the renegotiation extension, empty), certificate, done
        let mut body = vec![3, minor];
        body.extend(&server_random);
        body.push(0);
        body.extend(suite.to_be_bytes());
        body.push(0);
        body.extend([0, 5, 0xff, 0x01, 0, 1, 0]);
        let mut msgs = handshake_message(2, &body);
        let cert_len = SERVER_CERT.len() as u32;
        let list = [&(cert_len).to_be_bytes()[1..], SERVER_CERT].concat();
        msgs.extend(handshake_message(11, &[&(list.len() as u32).to_be_bytes()[1..], &list].concat()));
        msgs.extend(handshake_message(14, &[]));
        transcript.extend(&msgs);
        self.send(22, &msgs)?;
        // client key exchange: the premaster secret, RSA-encrypted (PKCS#1 v1.5 type 2)
        let (kind, exchange) = self.handshake()?;
        if kind != 16 || exchange.len() < 6 {
            return Err("no client key exchange".into());
        }
        transcript.extend(&exchange);
        let block = rsa_private(&exchange[6..], SERVER_N, SERVER_D);
        let premaster = block.iter().skip(2).position(|&b| b == 0).map(|z| block[z + 3..].to_vec()).filter(|p| p.len() == 48).ok_or("bad premaster secret")?;
        let master = prf(minor, &premaster, "master secret", &[&client_random[..], &server_random].concat(), 48);
        // MAC keys, cipher keys, then (TLS 1.0) the CBC IVs
        let keys = prf(minor, &master, "key expansion", &[&server_random[..], &client_random].concat(), 40 + 2 * key_len + 32);
        let (client_mac, server_mac) = (keys[..20].to_vec(), keys[20..40].to_vec());
        let (client_key, server_key) = (&keys[40..40 + key_len], &keys[40 + key_len..40 + 2 * key_len]);
        let ivs = &keys[40 + 2 * key_len..];
        let (client_iv, server_iv) = (ivs[..16].to_vec(), ivs[16..].to_vec());
        // change cipher spec, the client's finished message
        match self.record()? {
            (20, _) => {}
            (kind, _) => return Err(format!("record {kind} instead of change cipher spec")),
        }
        self.read = Some(Cipher { aes: Aes::new(client_key), mac_key: client_mac, seq: 0, minor, chain: client_iv });
        let expected = prf(minor, &master, "client finished", &transcript_hash(minor, &transcript), 12);
        let (kind, finished) = self.handshake()?;
        if kind != 20 || finished[4..] != expected[..] {
            return Err("bad client finished".into());
        }
        transcript.extend(&finished);
        self.send(20, &[1])?;
        self.write = Some(Cipher { aes: Aes::new(server_key), mac_key: server_mac, seq: 0, minor, chain: server_iv });
        let verify = prf(minor, &master, "server finished", &transcript_hash(minor, &transcript), 12);
        self.send(22, &handshake_message(20, &verify))
    }

    fn read_some(&mut self) -> Result<(), String> {
        loop {
            match self.record()? {
                (23, data) => {
                    self.data.extend(data);
                    return Ok(());
                }
                (22, _) => {}
                (kind, _) => return Err(format!("record {kind}")),
            }
        }
    }
}

fn handshake_message(kind: u8, body: &[u8]) -> Vec<u8> {
    [&[kind][..], &(body.len() as u32).to_be_bytes()[1..], body].concat()
}

// ── the billing answers ───────────────────────────────────────────────────────────────────

/// A form line's values.
fn form(line: &str) -> Vec<(String, String)> {
    line.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn answer(body: &[u8]) -> String {
    let text = miniz_oxide::inflate::decompress_to_vec(body).unwrap_or_else(|_| body.to_vec());
    let text = String::from_utf8_lossy(&text).into_owned();
    let cabinet = form(text.split("\r\n").next().unwrap_or(""));
    let value = |k: &str| cabinet.iter().find(|(key, _)| key == k).map_or("", |(_, v)| v.as_str()).to_string();
    let num = |k: &str| value(k).trim().parse::<i64>().unwrap_or(0);
    log!("billing: {}", text.split("\r\n").next().unwrap_or(""));
    if num("traceleft") > 0 {
        return "result=6&waittime=0&linelimit=20\r\n".into();
    }
    let (count, near) = (num("playcnt"), num("nearfull"));
    let mut limit = num("playlimit");
    while count >= limit - near {
        limit += 1024;
    }
    let nearfull = near | num("billingtype") << 16;
    let serial = value("keychipid");
    let limit_sig = sign(&[&(limit as u32).to_le_bytes()[..], serial.as_bytes()].concat());
    let near_sig = sign(&[&(nearfull as u32).to_le_bytes()[..], serial.as_bytes()].concat());
    let protocol = value("protocolver");
    format!(
        "result=0&requestno={}&traceerase=1&fixinterval=120&fixlogcnt=100&playlimit={limit}&playlimitsig={limit_sig}&playhistory=000000/0:000000/0:000000/0&nearfull={nearfull}&nearfullsig={near_sig}&linelimit=100&protocolver={}\r\n",
        value("requestno"),
        if protocol.is_empty() { "1.000".into() } else { protocol }
    )
}

fn client(tcp: TcpStream) -> Result<(), String> {
    let mut tls = Tls { tcp, minor: 3, read: None, write: None, pending: Vec::new(), data: Vec::new() };
    tls.accept()?;
    let header_end = loop {
        if let Some(i) = tls.data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        tls.read_some()?;
    };
    let head = String::from_utf8_lossy(&tls.data[..header_end]).into_owned();
    let length: usize = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse().ok()))
        .unwrap_or(0);
    while tls.data.len() < header_end + length {
        tls.read_some()?;
    }
    let body = answer(&tls.data[header_end..header_end + length]);
    log!("billing: -> {}", body.trim());
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    tls.send(23, response.as_bytes())?;
    // close notify
    tls.send(21, &[1, 0])
}

fn serve() {
    match TcpListener::bind(("127.0.0.1", port())) {
        Ok(listener) => {
            log!("billing: server on 127.0.0.1:{}", port());
            for tcp in listener.incoming().flatten() {
                log!("billing: connection");
                std::thread::spawn(move || {
                    if let Err(e) = client(tcp) {
                        log!("billing: {e}");
                    }
                });
            }
        }
        Err(e) => log!("billing: port {}: {e}", port()),
    }
}

pub(crate) fn init() {
    std::thread::spawn(serve);
}

// segatools' `pki/server.pem` (DER) and the RSA keys' modulus and private exponent (big
// endian) of `pki/server.key` and `pki/billing.key`

const SERVER_CERT: &[u8] = &[
    0x30, 0x82, 0x02, 0xb4, 0x30, 0x82, 0x01, 0x9c, 0x02, 0x01, 0x00, 0x30, 0x0d, 0x06, 0x09, 0x2a,
    0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b, 0x05, 0x00, 0x30, 0x25, 0x31, 0x10, 0x30, 0x0e,
    0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x07, 0x44, 0x75, 0x6d, 0x6d, 0x79, 0x43, 0x41, 0x31, 0x11,
    0x30, 0x0f, 0x06, 0x03, 0x55, 0x04, 0x0a, 0x0c, 0x08, 0x44, 0x75, 0x6d, 0x6d, 0x79, 0x50, 0x4b,
    0x49, 0x30, 0x20, 0x17, 0x0d, 0x31, 0x38, 0x31, 0x30, 0x31, 0x35, 0x31, 0x39, 0x33, 0x30, 0x35,
    0x31, 0x5a, 0x18, 0x0f, 0x32, 0x31, 0x31, 0x38, 0x31, 0x30, 0x31, 0x35, 0x31, 0x39, 0x33, 0x30,
    0x35, 0x31, 0x5a, 0x30, 0x19, 0x31, 0x17, 0x30, 0x15, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0e,
    0x69, 0x62, 0x2e, 0x6e, 0x61, 0x6f, 0x6d, 0x69, 0x6e, 0x65, 0x74, 0x2e, 0x6a, 0x70, 0x30, 0x82,
    0x01, 0x22, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05,
    0x00, 0x03, 0x82, 0x01, 0x0f, 0x00, 0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01, 0x00, 0xe0,
    0xd7, 0x1b, 0xe3, 0xf0, 0x92, 0xed, 0xe2, 0x70, 0xea, 0xad, 0x14, 0x0e, 0x1b, 0x30, 0xb2, 0xb3,
    0x46, 0xc3, 0xd9, 0x57, 0x46, 0x46, 0x74, 0x3f, 0x4d, 0x80, 0xb7, 0x7b, 0x38, 0x34, 0xc2, 0x16,
    0x1b, 0x41, 0x10, 0xfb, 0x6d, 0x47, 0x64, 0x35, 0x09, 0xb5, 0x29, 0xe3, 0x4e, 0x96, 0x3f, 0x32,
    0xaf, 0xc9, 0x23, 0xcf, 0x7d, 0x2c, 0xc0, 0x86, 0x3a, 0xe9, 0x47, 0xf0, 0xf3, 0x6a, 0x86, 0x1f,
    0x44, 0x23, 0xb2, 0x66, 0xd2, 0x39, 0x32, 0xeb, 0xd7, 0x70, 0x7d, 0x2b, 0x5a, 0x20, 0x77, 0x5d,
    0x5a, 0x33, 0x11, 0x2b, 0xbd, 0x59, 0xf1, 0xb4, 0xd0, 0x4e, 0x92, 0xec, 0x2d, 0xcc, 0xa2, 0xfe,
    0x65, 0xbd, 0x43, 0xf7, 0xd8, 0xef, 0xc5, 0x7d, 0x2e, 0x7b, 0x47, 0x48, 0xb0, 0x17, 0x2e, 0xa2,
    0x37, 0xa6, 0xca, 0x8c, 0xfa, 0x88, 0xd6, 0xd4, 0xed, 0x64, 0x8d, 0xb5, 0xb2, 0x05, 0x79, 0x01,
    0x6b, 0xf8, 0xc9, 0x59, 0xba, 0x31, 0xc8, 0xbf, 0x41, 0xe3, 0xef, 0xa3, 0x18, 0xc9, 0x90, 0x2a,
    0x36, 0xad, 0x85, 0xdc, 0xeb, 0xb5, 0x49, 0xdb, 0x95, 0x93, 0x0f, 0x80, 0x72, 0x89, 0x7c, 0x2b,
    0x34, 0xdf, 0x88, 0x82, 0x37, 0xf2, 0x26, 0xd9, 0x70, 0xff, 0x88, 0x36, 0x02, 0xba, 0x62, 0xf9,
    0xeb, 0x26, 0xbc, 0x5b, 0x0d, 0x9f, 0x8d, 0x5f, 0x0b, 0x17, 0x39, 0xe8, 0x34, 0xcb, 0xfb, 0xd9,
    0xee, 0x17, 0x36, 0x87, 0x59, 0x39, 0xc1, 0x3d, 0xc4, 0xc5, 0x6a, 0x32, 0x30, 0x3f, 0xdb, 0xbe,
    0x75, 0x6b, 0x29, 0x39, 0xde, 0x41, 0xfc, 0x0e, 0x23, 0xdb, 0x38, 0xb3, 0x7a, 0x41, 0xea, 0x8d,
    0x61, 0x19, 0xe5, 0x06, 0x46, 0x13, 0xef, 0xa4, 0x9d, 0xb4, 0xf8, 0x51, 0x8e, 0x13, 0xc2, 0xc8,
    0x47, 0x30, 0x44, 0x8b, 0x10, 0xca, 0x97, 0xcd, 0x93, 0x1b, 0xad, 0xc3, 0x56, 0x83, 0x09, 0x02,
    0x03, 0x01, 0x00, 0x01, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01,
    0x0b, 0x05, 0x00, 0x03, 0x82, 0x01, 0x01, 0x00, 0x0d, 0x3a, 0xa1, 0x6a, 0x8e, 0x71, 0x9f, 0xf3,
    0x4b, 0xef, 0xc1, 0x81, 0x24, 0x52, 0xc7, 0xce, 0x5a, 0x9f, 0x2b, 0x35, 0x56, 0xad, 0xfb, 0xca,
    0xdd, 0x5e, 0x5d, 0x01, 0x4a, 0x11, 0x81, 0x5e, 0x20, 0xc8, 0xb7, 0x33, 0x0e, 0x53, 0xbf, 0x4a,
    0x62, 0x43, 0xce, 0x6e, 0x78, 0x90, 0xe5, 0x12, 0xc1, 0x32, 0x1a, 0xef, 0xaa, 0x50, 0xac, 0x4a,
    0x4d, 0x0d, 0xa7, 0xe6, 0x60, 0xbd, 0x5d, 0x76, 0x27, 0x85, 0xad, 0x99, 0xff, 0xf3, 0x9c, 0xb6,
    0xb9, 0x64, 0xac, 0x9e, 0x23, 0x83, 0x75, 0x14, 0x27, 0x9f, 0x53, 0xd7, 0x25, 0xe3, 0x1a, 0x41,
    0xd3, 0xc1, 0x6f, 0xf1, 0x85, 0x9b, 0x61, 0x5e, 0xd2, 0x30, 0xda, 0xf9, 0xe1, 0x99, 0x14, 0x45,
    0xd8, 0x68, 0xe0, 0x55, 0x51, 0xd6, 0x90, 0x27, 0xa6, 0xe8, 0xad, 0xb6, 0x90, 0x04, 0x8c, 0xc4,
    0xf8, 0x47, 0x8b, 0x9b, 0x6c, 0xe1, 0xfc, 0x50, 0x46, 0xcc, 0x4d, 0xb5, 0x60, 0xc5, 0x77, 0x67,
    0xd3, 0xce, 0x88, 0x02, 0x57, 0xec, 0x62, 0x7d, 0x20, 0x9c, 0xe9, 0x5f, 0xef, 0xa4, 0x64, 0x24,
    0x52, 0xde, 0xe1, 0x49, 0x56, 0x69, 0x90, 0x7b, 0xe5, 0x26, 0x9a, 0x5e, 0x73, 0x5a, 0xbc, 0x13,
    0xcd, 0xf6, 0x21, 0x67, 0xa9, 0x72, 0xdc, 0x92, 0x0e, 0xad, 0xb0, 0x55, 0x09, 0x9c, 0xd3, 0x02,
    0x7a, 0xca, 0xb9, 0x68, 0x74, 0x3d, 0xb1, 0x75, 0xac, 0xe2, 0x82, 0xa9, 0x26, 0x2f, 0x12, 0x5f,
    0x1b, 0x7f, 0xcc, 0xd4, 0x66, 0x81, 0x39, 0xd0, 0x2b, 0x63, 0x8d, 0x33, 0x7f, 0x0d, 0x79, 0x9a,
    0x4b, 0x9a, 0x55, 0x8f, 0x93, 0x23, 0x71, 0xce, 0xc4, 0x4d, 0x95, 0x85, 0xa8, 0xfa, 0x45, 0x8f,
    0x0b, 0xdf, 0x23, 0x54, 0x57, 0xb5, 0xfb, 0x72, 0xbc, 0xf0, 0x84, 0xa4, 0xd0, 0x34, 0x8d, 0xe1,
    0x73, 0x90, 0x42, 0x7a, 0xc8, 0xbe, 0x7e, 0xbc,
];

const SERVER_N: &[u8] = &[
    0xe0, 0xd7, 0x1b, 0xe3, 0xf0, 0x92, 0xed, 0xe2, 0x70, 0xea, 0xad, 0x14, 0x0e, 0x1b, 0x30, 0xb2,
    0xb3, 0x46, 0xc3, 0xd9, 0x57, 0x46, 0x46, 0x74, 0x3f, 0x4d, 0x80, 0xb7, 0x7b, 0x38, 0x34, 0xc2,
    0x16, 0x1b, 0x41, 0x10, 0xfb, 0x6d, 0x47, 0x64, 0x35, 0x09, 0xb5, 0x29, 0xe3, 0x4e, 0x96, 0x3f,
    0x32, 0xaf, 0xc9, 0x23, 0xcf, 0x7d, 0x2c, 0xc0, 0x86, 0x3a, 0xe9, 0x47, 0xf0, 0xf3, 0x6a, 0x86,
    0x1f, 0x44, 0x23, 0xb2, 0x66, 0xd2, 0x39, 0x32, 0xeb, 0xd7, 0x70, 0x7d, 0x2b, 0x5a, 0x20, 0x77,
    0x5d, 0x5a, 0x33, 0x11, 0x2b, 0xbd, 0x59, 0xf1, 0xb4, 0xd0, 0x4e, 0x92, 0xec, 0x2d, 0xcc, 0xa2,
    0xfe, 0x65, 0xbd, 0x43, 0xf7, 0xd8, 0xef, 0xc5, 0x7d, 0x2e, 0x7b, 0x47, 0x48, 0xb0, 0x17, 0x2e,
    0xa2, 0x37, 0xa6, 0xca, 0x8c, 0xfa, 0x88, 0xd6, 0xd4, 0xed, 0x64, 0x8d, 0xb5, 0xb2, 0x05, 0x79,
    0x01, 0x6b, 0xf8, 0xc9, 0x59, 0xba, 0x31, 0xc8, 0xbf, 0x41, 0xe3, 0xef, 0xa3, 0x18, 0xc9, 0x90,
    0x2a, 0x36, 0xad, 0x85, 0xdc, 0xeb, 0xb5, 0x49, 0xdb, 0x95, 0x93, 0x0f, 0x80, 0x72, 0x89, 0x7c,
    0x2b, 0x34, 0xdf, 0x88, 0x82, 0x37, 0xf2, 0x26, 0xd9, 0x70, 0xff, 0x88, 0x36, 0x02, 0xba, 0x62,
    0xf9, 0xeb, 0x26, 0xbc, 0x5b, 0x0d, 0x9f, 0x8d, 0x5f, 0x0b, 0x17, 0x39, 0xe8, 0x34, 0xcb, 0xfb,
    0xd9, 0xee, 0x17, 0x36, 0x87, 0x59, 0x39, 0xc1, 0x3d, 0xc4, 0xc5, 0x6a, 0x32, 0x30, 0x3f, 0xdb,
    0xbe, 0x75, 0x6b, 0x29, 0x39, 0xde, 0x41, 0xfc, 0x0e, 0x23, 0xdb, 0x38, 0xb3, 0x7a, 0x41, 0xea,
    0x8d, 0x61, 0x19, 0xe5, 0x06, 0x46, 0x13, 0xef, 0xa4, 0x9d, 0xb4, 0xf8, 0x51, 0x8e, 0x13, 0xc2,
    0xc8, 0x47, 0x30, 0x44, 0x8b, 0x10, 0xca, 0x97, 0xcd, 0x93, 0x1b, 0xad, 0xc3, 0x56, 0x83, 0x09,
];

const SERVER_D: &[u8] = &[
    0xd2, 0x95, 0x05, 0xfd, 0x3e, 0x04, 0x0f, 0x23, 0x67, 0x88, 0x94, 0xa4, 0x54, 0xf5, 0xe8, 0x6d,
    0x66, 0x75, 0x9f, 0xd6, 0xfe, 0xe3, 0xc5, 0xf6, 0xf8, 0x97, 0x19, 0x38, 0x40, 0x03, 0x0b, 0x7e,
    0xc0, 0x7f, 0xb4, 0x38, 0x38, 0x94, 0x48, 0x2a, 0x35, 0x76, 0xab, 0x23, 0xd9, 0x4e, 0x70, 0x70,
    0x2e, 0x83, 0xef, 0x8a, 0x47, 0xba, 0x40, 0x60, 0xed, 0x17, 0x5e, 0x08, 0x50, 0x47, 0x5b, 0x7c,
    0xd4, 0xe9, 0xd7, 0xd9, 0x2e, 0x6e, 0x34, 0xbe, 0x59, 0x9f, 0x45, 0x2f, 0x26, 0x9c, 0x5a, 0x0e,
    0x11, 0xf3, 0xfa, 0x22, 0x59, 0xea, 0x15, 0xc4, 0xc9, 0x09, 0xd4, 0x9d, 0xcc, 0xf0, 0xb4, 0x59,
    0xf0, 0x94, 0xcf, 0x68, 0x4a, 0x48, 0xf7, 0x80, 0xeb, 0x56, 0xce, 0x85, 0x9c, 0x31, 0xb8, 0xf4,
    0xa7, 0x60, 0x22, 0x2a, 0x51, 0x52, 0x11, 0x39, 0xcc, 0xe1, 0xae, 0xac, 0xff, 0x01, 0x06, 0x17,
    0x66, 0xeb, 0x98, 0xee, 0xeb, 0x74, 0x87, 0xa5, 0xf5, 0xd7, 0x1a, 0x1d, 0x75, 0xe7, 0x93, 0x39,
    0xd7, 0xdd, 0xfa, 0xb9, 0x2d, 0xc4, 0xf5, 0x69, 0x1d, 0xe2, 0x43, 0x73, 0x3f, 0x45, 0xe8, 0x69,
    0x3f, 0xd2, 0x98, 0x35, 0x7b, 0x3b, 0x3a, 0xe9, 0x36, 0x79, 0xb1, 0x6b, 0x74, 0xfd, 0x98, 0x4f,
    0x65, 0x64, 0xc1, 0xe3, 0x42, 0x8f, 0x79, 0xcd, 0xa5, 0xa2, 0x64, 0x46, 0xc1, 0x8d, 0xfa, 0x00,
    0xc3, 0x93, 0xde, 0xe1, 0x6e, 0x72, 0x76, 0xed, 0x26, 0x0c, 0x61, 0x4c, 0x67, 0x6a, 0xb1, 0x1b,
    0xfb, 0x2a, 0x5d, 0x31, 0x09, 0x3a, 0x3b, 0xc5, 0x6f, 0xfc, 0xe5, 0x96, 0xbb, 0xae, 0x15, 0xca,
    0xfb, 0x14, 0x5d, 0x1e, 0xb6, 0x2b, 0xf7, 0x0e, 0xb1, 0x61, 0x32, 0x87, 0x8b, 0xd0, 0x29, 0xe0,
    0x71, 0x26, 0x51, 0xcc, 0x7f, 0x7c, 0x60, 0x2d, 0x75, 0xdc, 0x6b, 0xd1, 0xfc, 0xdd, 0xa4, 0x81,
];

const BILLING_N: &[u8] = &[
    0xc2, 0xf6, 0x18, 0xf5, 0x2e, 0xce, 0xfd, 0x8d, 0xe1, 0xb6, 0x02, 0xc6, 0xd7, 0x39, 0x6d, 0x94,
    0xb3, 0xa2, 0x51, 0xc5, 0x59, 0xd0, 0x94, 0x6c, 0xff, 0xb0, 0x86, 0x3a, 0xf7, 0x2b, 0xdf, 0x05,
    0x38, 0xb9, 0xd8, 0x7a, 0x88, 0xbb, 0x9a, 0xd5, 0x12, 0x51, 0x46, 0x0e, 0x5b, 0xcc, 0x08, 0xab,
    0x2e, 0x96, 0x33, 0x77, 0x38, 0x73, 0x40, 0x69, 0xec, 0x57, 0x10, 0x2e, 0x76, 0x74, 0xcd, 0x63,
    0x26, 0xd5, 0x37, 0xcb, 0x18, 0x76, 0x73, 0x66, 0x69, 0x39, 0x9f, 0xb2, 0x2f, 0xe8, 0x70, 0xcf,
    0xaf, 0x77, 0xa1, 0x0e, 0x06, 0xd6, 0x2e, 0x73, 0xa9, 0x46, 0xb5, 0x6a, 0x64, 0x3c, 0x69, 0x00,
    0x60, 0xf6, 0xbd, 0x06, 0xf8, 0xa7, 0x60, 0xd6, 0xe0, 0x8d, 0xb1, 0x19, 0x4a, 0xa7, 0xa8, 0x36,
    0xbe, 0xf8, 0x2f, 0x1d, 0x1f, 0xfd, 0x25, 0x5e, 0xa1, 0x3a, 0x72, 0xac, 0x06, 0x5c, 0xd1, 0xad,
];

const BILLING_D: &[u8] = &[
    0x8b, 0x38, 0x0f, 0x84, 0x6e, 0x03, 0x93, 0x9e, 0x50, 0x92, 0x97, 0x93, 0x04, 0x37, 0xd7, 0x1f,
    0x7b, 0xd3, 0x2a, 0x8f, 0x41, 0xde, 0xa7, 0x4e, 0xda, 0x9b, 0x64, 0x8f, 0x10, 0xd7, 0xfc, 0x2f,
    0x0f, 0x5d, 0xee, 0xcf, 0x39, 0x7a, 0xb6, 0x4a, 0xf7, 0x04, 0xa0, 0x81, 0x3a, 0x5e, 0x0d, 0xc6,
    0xa7, 0xc2, 0xcc, 0x47, 0x93, 0x41, 0x01, 0xbb, 0x61, 0xf0, 0x9d, 0xf6, 0x7f, 0xd2, 0x52, 0xa2,
    0xfd, 0xcf, 0x4a, 0x21, 0x82, 0x1a, 0x7a, 0x38, 0x93, 0x68, 0xb6, 0xb3, 0x78, 0xd1, 0xb9, 0x58,
    0x3b, 0xe8, 0x51, 0x8b, 0x0e, 0xc6, 0x47, 0xfc, 0xdd, 0x5d, 0xb7, 0x88, 0x7b, 0xf1, 0x59, 0xd6,
    0xd1, 0x59, 0x48, 0x9f, 0x06, 0x8c, 0xe9, 0xb4, 0x2f, 0xdf, 0x3a, 0x74, 0xbb, 0xca, 0xd7, 0x56,
    0x4f, 0xcf, 0xc2, 0x96, 0x21, 0xf4, 0x01, 0xa6, 0xaa, 0xe0, 0x49, 0x4b, 0x75, 0x58, 0x51, 0xc1,
];
