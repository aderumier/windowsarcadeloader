//! The game's ABAAS game server (Sega World Drivers Championship's player data: Aime card
//! logins, saves, rankings): the game posts to `http://<ALL.Net's uri>:80/api/...` (the uri a
//! bare host name, `WAL_ALLS_TITLE_URI`), resolved to the virtual router; the game's connections
//! to its port 80 go to this server in the game's own process, 127.0.0.1:`WAL_PORT`+104
//! (`allnet.rs`), answered by payload-common's `abaas_gs` (the players' files in `Y:\gs\`).
//! The title key comes from the game's `BCryptGenerateSymmetricKey` call.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{abaas_gs, iat, log};

pub(crate) fn port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 104
}

static GENERATE_KEY: AtomicUsize = AtomicUsize::new(0);

type GenerateKey = unsafe extern "system" fn(usize, *mut usize, *mut u8, u32, *const u8, u32, u32) -> i32;

unsafe extern "system" fn generate_key(alg: usize, key: *mut usize, object: *mut u8, object_len: u32, secret: *const u8, secret_len: u32, flags: u32) -> i32 {
    if !secret.is_null() {
        // a 256-bit key: the title key
        abaas_gs::learn_key(unsafe { std::slice::from_raw_parts(secret, secret_len as usize) });
    }
    let real: GenerateKey = unsafe { std::mem::transmute(GENERATE_KEY.load(Ordering::Relaxed)) };
    unsafe { real(alg, key, object, object_len, secret, secret_len, flags) }
}

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
    // `/api/...` (after any prefix)
    let api = path.find("/api/").map_or(path.as_str(), |at| &path[at..]);
    let reply = abaas_gs::reply(api, &body);
    let mut out = stream;
    out.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).as_bytes())?;
    out.write_all(&reply)
}

fn serve() {
    match TcpListener::bind(("127.0.0.1", port())) {
        Ok(listener) => {
            log!("gs: server on 127.0.0.1:{}", port());
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let _ = client(stream);
                });
            }
        }
        Err(e) => log!("gs: 127.0.0.1:{}: {e}", port()),
    }
}

pub(crate) fn init() {
    abaas_gs::configure("Y:\\gs", "WAL_ALLS");
    match unsafe { iat::hook("bcrypt.dll", "BCryptGenerateSymmetricKey", generate_key as *const () as usize) } {
        Some(orig) => GENERATE_KEY.store(orig, Ordering::Relaxed),
        None => log!("gs: BCryptGenerateSymmetricKey not imported"),
    }
    std::thread::spawn(serve);
}
