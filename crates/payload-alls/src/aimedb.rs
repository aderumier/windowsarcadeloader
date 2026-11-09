//! AimeDB, the Aime card database amdaemon asks for a card's Aime id (TCP 22345 of
//! `aime.naominet.jp`, the virtual router in the payload's DNS): answered by the payload on
//! 127.0.0.1:`WAL_PORT`+103 (`allnet.rs` moves the connections). Formats from ARTEMiS'
//! `core/aimedb.py` and `core/adb_handlers` (WTFPL).
//!
//! Messages are AES-128-ECB encrypted (key `Copyright(C)SEGA`): a 0x20-byte header (magic
//! 0xa13e, protocol version, command, length, status, game id[6], store id, keychip id[12]),
//! then the command's body. The answer's command is the request's answer code, status 1 (good).
//! - 0x64 hello, 0x07 status log, 0x09 log: a header
//! - 0x04 lookup / 0x05 register (access code, 10 BCD bytes at 0x20): the Aime id, 0x0F lookup
//!   ex the same with an empty auth key: the id is 1000000000 plus the access code's last 9
//!   digits (`WAL_ALLS_AIME_ID` instead; ten digits: Sega World Drivers Championship ends the
//!   play of a user id under nine characters, "user id is invalid"); 0x01 / 0x11 FeliCa lookups: the card's IDm as access code
//! - 0x0B campaigns: none; 0x0D campaign clear status: none; 0x13 log ex: every log accepted
//! - 0x66 goodbye: the connection closed

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::log;

const KEY: &[u8; 16] = b"Copyright(C)SEGA";
const HEADER: usize = 0x20;

pub(crate) fn port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 103
}

fn crypt(data: &mut [u8], encrypt: bool) {
    let cipher = Aes128::new(GenericArray::from_slice(KEY));
    for block in data.chunks_exact_mut(16) {
        let b = GenericArray::from_mut_slice(block);
        if encrypt { cipher.encrypt_block(b) } else { cipher.decrypt_block(b) }
    }
}

/// The card's Aime id: `WAL_ALLS_AIME_ID`, else 1000000000 plus the access code's last 9
/// digits (ten digits, a positive 32-bit number).
pub(crate) fn aime_id(access_code: &str) -> i32 {
    if let Some(id) = std::env::var("WAL_ALLS_AIME_ID").ok().and_then(|v| v.trim().parse::<i32>().ok()) {
        return id;
    }
    1_000_000_000 + access_code.get(11..).and_then(|d| d.parse::<i32>().ok()).unwrap_or(1)
}

/// An answer: the request's header with the answer's command, length and status good, then
/// `body` (padded to `length`).
fn answer(req: &[u8], cmd: u16, length: usize, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; length];
    out[..HEADER].copy_from_slice(&req[..HEADER]);
    out[4..6].copy_from_slice(&cmd.to_le_bytes());
    out[6..8].copy_from_slice(&(length as u16).to_le_bytes());
    out[8..10].copy_from_slice(&1u16.to_le_bytes());
    out[HEADER..HEADER + body.len()].copy_from_slice(body);
    out
}

fn access_code(req: &[u8]) -> String {
    req.get(HEADER..HEADER + 10).map(|b| b.iter().map(|x| format!("{x:02x}")).collect()).unwrap_or_default()
}

/// The answer to one decrypted request (`None`: goodbye).
fn handle(req: &[u8]) -> Option<Vec<u8>> {
    let cmd = u16::from_le_bytes([req[4], req[5]]);
    if std::env::var("WAL_ALLS_AIMEDB_DEBUG").is_ok() {
        log!("aimedb: request {cmd:#x}");
    }
    Some(match cmd {
        0x66 => return None,
        0x04 | 0x05 | 0x0F => {
            let code = access_code(req);
            let id = aime_id(&code);
            log!("aimedb: card {code}: Aime id {id}");
            let mut body = [0u8; 0x110];
            body[..4].copy_from_slice(&id.to_le_bytes());
            if cmd == 0x0F {
                // portal registration, padding, the auth key (256 bytes), relations -1
                body[0x108..0x10c].copy_from_slice(&(-1i32).to_le_bytes());
                body[0x10c..0x110].copy_from_slice(&(-1i32).to_le_bytes());
                answer(req, 0x10, 0x130, &body)
            } else {
                answer(req, 0x06, 0x30, &body[..0x10])
            }
        }
        0x01 | 0x02 | 0x11 => {
            // FeliCa: the IDm (at 0x20; lookup ex: 0x30, after 16 random bytes) as the access
            // code (zero padded)
            let at = if cmd == 0x11 { 0x30 } else { HEADER };
            let mut code = [0u8; 10];
            code[2..].copy_from_slice(&req[at..at + 8]);
            if cmd == 0x11 {
                // Aime id, relations -1, the access code, portal, company, the auth key
                let digits: String = code.iter().map(|x| format!("{x:02x}")).collect();
                let mut body = [0u8; 0x110];
                body[..4].copy_from_slice(&aime_id(&digits).to_le_bytes());
                body[4..8].copy_from_slice(&(-1i32).to_le_bytes());
                body[8..12].copy_from_slice(&(-1i32).to_le_bytes());
                body[12..22].copy_from_slice(&code);
                answer(req, 0x12, 0x130, &body)
            } else {
                // FeliCa index, the access code
                let mut body = [0u8; 0x10];
                body[4..14].copy_from_slice(&code);
                answer(req, 0x03, 0x30, &body)
            }
        }
        0x0B => {
            let version = u16::from_le_bytes([req[2], req[3]]);
            if version >= 0x3030 { answer(req, 0x0C, 0x200, &[]) } else { answer(req, 0x0C, 0x30, &[]) }
        }
        0x0D => answer(req, 0x0E, 0x50, &[]),
        0x13 => answer(req, 0x14, 0x40, &[1; 20]),
        0x07 => answer(req, 0x08, HEADER, &[]),
        0x09 => answer(req, 0x0A, HEADER, &[]),
        0x64 => answer(req, 0x65, HEADER, &[]),
        _ => {
            log!("aimedb: command {cmd:#x} ({} bytes)", req.len());
            answer(req, cmd + 1, HEADER, &[])
        }
    })
}

fn client(mut stream: TcpStream) {
    loop {
        let mut head = [0u8; HEADER];
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        let mut plain = head;
        crypt(&mut plain, false);
        if u16::from_le_bytes([plain[0], plain[1]]) != 0xa13e {
            return log!("aimedb: bad message");
        }
        let length = (u16::from_le_bytes([plain[6], plain[7]]) as usize).max(HEADER);
        let mut req = head.to_vec();
        req.resize(length.div_ceil(16) * 16, 0);
        if stream.read_exact(&mut req[HEADER..]).is_err() {
            return;
        }
        crypt(&mut req, false);
        let Some(mut out) = handle(&req) else { return };
        out.resize(out.len().div_ceil(16) * 16, 0);
        crypt(&mut out, true);
        if stream.write_all(&out).is_err() {
            return;
        }
    }
}

fn serve() {
    match TcpListener::bind(("127.0.0.1", port())) {
        Ok(listener) => {
            log!("aimedb: server on 127.0.0.1:{}", port());
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || client(stream));
            }
        }
        Err(e) => log!("aimedb: 127.0.0.1:{}: {e}", port()),
    }
}

pub(crate) fn init() {
    std::thread::spawn(serve);
}
