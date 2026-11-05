//! The APM3 title server (`apms-api`): the players' data of Aime cards. The game's `libcts`
//! client posts to `http://<ip>:80/apms-api/<request>` (StartGame, EndGame, ContinueGame,
//! UpdateUser, Heartbeat, GetRanking, Get/SetCollection, Alive, CreateAccessCode, ConvertCtsId,
//! SetCabinetProperty), the address resolved from `<keychip>.apm.sys-all.net` (`DnsQuery_A`),
//! only when ALL.Net is good. Bodies: a 16-byte header, then zlib-compressed MessagePack,
//! AES-CBC encrypted (`bcrypt.dll`).
//!
//! Here: that name resolves to 127.0.0.1, the game's port 80 connections go to the payload's
//! server on [`port`] (port 80 needs root on the host): the launcher's port + 100 (33800 by
//! default), one per launcher (the parallel test runs use other launcher ports).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;

use wal_payload_common::{iat, log};

/// The title server's port: the launcher's port + 100.
pub(crate) fn port() -> u16 {
    static PORT: OnceLock<u16> = OnceLock::new();
    *PORT.get_or_init(|| {
        let launcher = std::env::var(wal_protocol::env::PORT).ok().and_then(|p| p.parse().ok()).unwrap_or(wal_protocol::DEFAULT_PORT);
        launcher.wrapping_add(100)
    })
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- DNS: the title server's name ----

type DnsQueryA = unsafe extern "system" fn(*const u8, u16, u32, *mut u8, *mut *mut u8, *mut u8) -> i32;
static REAL_DNS: OnceLock<usize> = OnceLock::new();

/// `DNS_RECORDA` (32-bit): pNext, pName, wType, wDataLength, Flags, dwTtl, dwReserved, A address.
#[repr(C)]
struct DnsRecordA {
    next: usize,
    name: *const u8,
    kind: u16,
    data_length: u16,
    flags: u32,
    ttl: u32,
    reserved: u32,
    address: u32,
}

unsafe extern "system" fn dns_query_a(name: *const u8, kind: u16, options: u32, extra: *mut u8, results: *mut *mut u8, reserved: *mut u8) -> i32 {
    let text = if name.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned() };
    if text.to_ascii_lowercase().ends_with("apm.sys-all.net") && !results.is_null() {
        log!("apms: DNS {text}: 127.0.0.1");
        let record = Box::leak(Box::new(DnsRecordA {
            next: 0,
            name,
            kind: 1,
            data_length: 4,
            flags: 0,
            ttl: 3600,
            reserved: 0,
            address: u32::from_ne_bytes([127, 0, 0, 1]),
        }));
        unsafe { *results = (record as *mut DnsRecordA).cast() };
        return 0;
    }
    let real: DnsQueryA = unsafe { std::mem::transmute(*REAL_DNS.get().unwrap()) };
    unsafe { real(name, kind, options, extra, results, reserved) }
}

// ---- connect: port 80 of 127.0.0.1 to the payload's server ----

type Connect = unsafe extern "system" fn(usize, *const u8, i32) -> i32;
static REAL_CONNECT: OnceLock<usize> = OnceLock::new();

unsafe extern "system" fn connect(socket: usize, address: *const u8, length: i32) -> i32 {
    let real: Connect = unsafe { std::mem::transmute(*REAL_CONNECT.get().unwrap()) };
    if !address.is_null() && length >= 16 {
        // sockaddr_in: family, port (big endian), address
        let a = unsafe { std::slice::from_raw_parts(address, 16) };
        if a[0] == 2 && a[2..4] == [0, 80] && a[4..8] == [127, 0, 0, 1] {
            let mut moved = [0u8; 16];
            moved.copy_from_slice(a);
            moved[2..4].copy_from_slice(&port().to_be_bytes());
            return unsafe { real(socket, moved.as_ptr(), length) };
        }
    }
    unsafe { real(socket, address, length) }
}

// ---- bcrypt: the AES key and the plain bodies (WAL_APM3_APMS_TRACE=1) ----

type GenerateKey = unsafe extern "system" fn(usize, *mut usize, *mut u8, u32, *const u8, u32, u32) -> i32;
type Crypt = unsafe extern "system" fn(usize, *const u8, u32, *const u8, *mut u8, u32, *mut u8, u32, *mut u32, u32) -> i32;
static REAL_GENERATE: OnceLock<usize> = OnceLock::new();
static REAL_ENCRYPT: OnceLock<usize> = OnceLock::new();
static REAL_DECRYPT: OnceLock<usize> = OnceLock::new();

unsafe extern "system" fn generate_key(alg: usize, key: *mut usize, object: *mut u8, object_len: u32, secret: *const u8, secret_len: u32, flags: u32) -> i32 {
    if !secret.is_null() {
        log!("apms: key {}", hex(unsafe { std::slice::from_raw_parts(secret, secret_len as usize) }));
    }
    let real: GenerateKey = unsafe { std::mem::transmute(*REAL_GENERATE.get().unwrap()) };
    unsafe { real(alg, key, object, object_len, secret, secret_len, flags) }
}

unsafe extern "system" fn encrypt(key: usize, input: *const u8, input_len: u32, padding: *const u8, iv: *mut u8, iv_len: u32, output: *mut u8, output_len: u32, result: *mut u32, flags: u32) -> i32 {
    if !output.is_null() && !input.is_null() {
        let iv_hex = if iv.is_null() { String::new() } else { hex(unsafe { std::slice::from_raw_parts(iv, iv_len as usize) }) };
        log!("apms: encrypt iv {iv_hex} plain {}", hex(unsafe { std::slice::from_raw_parts(input, input_len as usize) }));
    }
    let real: Crypt = unsafe { std::mem::transmute(*REAL_ENCRYPT.get().unwrap()) };
    unsafe { real(key, input, input_len, padding, iv, iv_len, output, output_len, result, flags) }
}

unsafe extern "system" fn decrypt(key: usize, input: *const u8, input_len: u32, padding: *const u8, iv: *mut u8, iv_len: u32, output: *mut u8, output_len: u32, result: *mut u32, flags: u32) -> i32 {
    let real: Crypt = unsafe { std::mem::transmute(*REAL_DECRYPT.get().unwrap()) };
    let status = unsafe { real(key, input, input_len, padding, iv, iv_len, output, output_len, result, flags) };
    if !output.is_null() && !result.is_null() && status == 0 {
        log!("apms: decrypt plain {}", hex(unsafe { std::slice::from_raw_parts(output, *result as usize) }));
    }
    status
}

// ---- the server ----

use crate::cts::{self, Value, arr, int, s};

/// The players' records: `apms/<aime id>.dat` in the game's folder, the MessagePack
/// `[profile, slots]` of the player's last EndGame: profile `[name, u16, save block]` (the
/// game's own save, checked by the game), slots the 16 values after it.
const SAVE_DIR: &str = "apms";
/// The game's save block size (Under Night copies 0x4000 bytes; it sends 0x1c00).
const SAVE_BLOCK: usize = 0x4000;

fn record_path(aime_id: i64) -> String {
    format!("{SAVE_DIR}\\{aime_id}.dat")
}

fn load_record(aime_id: i64) -> Option<(Value, Value)> {
    let data = std::fs::read(record_path(aime_id)).ok()?;
    let record = cts::decode_all(&data).into_iter().next()?;
    Some((record.get(0).clone(), record.get(1).clone()))
}

fn save_record(aime_id: i64, profile: &Value, slots: &Value) {
    let _ = std::fs::create_dir_all(SAVE_DIR);
    let data = arr(vec![profile.clone(), slots.clone()]).encode();
    match std::fs::write(record_path(aime_id), data) {
        Ok(()) => log!("apms: player {aime_id} saved"),
        Err(e) => log!("apms: player {aime_id}: {e}"),
    }
}

/// A profile `[name, u16, save block]` with its block at least [`SAVE_BLOCK`] bytes (the game
/// copies that much).
fn padded(profile: &Value) -> Value {
    let mut block = profile.get(2).bytes().unwrap_or(&[]).to_vec();
    if block.len() < SAVE_BLOCK {
        block.resize(SAVE_BLOCK, 0);
    }
    arr(vec![profile.get(0).clone(), profile.get(1).clone(), Value::Str(block)])
}

/// The answer to one request: status and data (none: the header alone).
fn answer(api: &str, request: &Value) -> (u16, Option<Value>) {
    let args = request.array();
    let aime_id = request.get(1).get(0).int().unwrap_or(-1);
    match api {
        // [header, [aime id, ""], [first slot, slots]] -> [[aime id, ""], profile, slots]; a new
        // player: a zero save block (the game makes a new save)
        "StartGame" => {
            let (profile, slots) = load_record(aime_id).unwrap_or_else(|| (arr(vec![s(""), int(0), Value::Str(vec![])]), arr(vec![])));
            log!("apms: player {aime_id}: {}", if slots.array().is_empty() { "new" } else { "known" });
            (0, Some(arr(vec![arr(vec![int(aime_id), s("")]), padded(&profile), slots])))
        }
        // [header, [aime id, ""], profile, slots, play logs]
        "EndGame" | "ContinueGame" | "UpdateUser" => {
            let profile = request.get(2);
            if aime_id >= 0 && profile.get(2).bytes().is_some_and(|b| !b.is_empty()) {
                save_record(aime_id, profile, request.get(3));
            }
            (0, None)
        }
        // [header, [kind, [ranking ids], filters]] -> [[ranking...]]: every ranking empty
        "GetRanking" => {
            let ids = request.get(1).get(1).array().len();
            (0, Some(arr(vec![arr(vec![arr(vec![]); ids])])))
        }
        "GetCollection" => (0, Some(arr(vec![]))),
        _ => {
            let _ = args;
            (0, None)
        }
    }
}

fn serve(stream: TcpStream) -> std::io::Result<()> {
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
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let api = path.rsplit('/').next().unwrap_or("").to_string();
    let request = cts::open_request(&body).map(|raw| cts::decode_all(&raw).into_iter().next().unwrap_or(Value::Nil)).unwrap_or(Value::Nil);
    log!("apms: {api} {}", cts::show(&request));
    let (status, data) = answer(&api, &request);
    if let Some(d) = &data {
        log!("apms: {api} -> {status} {}", cts::show(d));
    }
    let reply = cts::seal_response(status, data.map(|d| d.encode()).as_deref());
    let mut out = stream;
    out.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).as_bytes())?;
    out.write_all(&reply)?;
    Ok(())
}

pub(crate) fn init() {
    let base = iat::game_module();
    unsafe {
        if let Some(real) = iat::hook("DNSAPI.dll", "DnsQuery_A", dns_query_a as *const () as usize) {
            let _ = REAL_DNS.set(real);
        }
        if let Some(real) = iat::hook_ordinal(base, "WS2_32.dll", 4, connect as *const () as usize) {
            let _ = REAL_CONNECT.set(real);
        }
        if std::env::var("WAL_APM3_APMS_TRACE").is_ok_and(|v| v.trim() == "1") {
            for (name, f, slot) in [
                ("BCryptGenerateSymmetricKey", generate_key as *const () as usize, &REAL_GENERATE),
                ("BCryptEncrypt", encrypt as *const () as usize, &REAL_ENCRYPT),
                ("BCryptDecrypt", decrypt as *const () as usize, &REAL_DECRYPT),
            ] {
                if let Some(real) = iat::hook("bcrypt.dll", name, f) {
                    let _ = slot.set(real);
                }
            }
        }
    }
    match TcpListener::bind(("127.0.0.1", port())) {
        Ok(listener) => {
            log!("apms: title server on 127.0.0.1:{}", port());
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || {
                        if let Err(e) = serve(stream) {
                            log!("apms: {e}");
                        }
                    });
                }
            });
        }
        Err(e) => log!("apms: 127.0.0.1:{}: {e}", port()),
    }
}
