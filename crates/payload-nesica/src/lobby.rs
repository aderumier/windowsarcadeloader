//! The online lobby server of Left 4 Dead: Survivors, answered with no rooms.
//!
//! With a NESiCA card logged in, the game looks for other cabinets' rooms before a stage: its
//! engine sends UDP queries to its card server host (222.158.239.24, a real address) on port
//! 27010, from its game port + 1000, and gives up 5 s later ("the connection timed out", back to
//! the mode select). A query is `<protocol><0>` and `\`-separated fields (`resend\<n>`,
//! protocol `2`: `seqNo\<n>` first). The answer of protocol `1` is an error byte then the
//! servers; of protocol `2` the query's sequence number (big endian), the error byte, the server
//! count and index, the servers. Error 0x7E (as 0x7A): an empty list.
//!
//! `WAL_NESICA_LOBBY=<port>` (27010): the engine's `sendto` to that port goes to the payload's
//! server on 127.0.0.1, which answers every query with an empty list. Each query is logged.

use std::net::UdpSocket;

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

static PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
static ORIG_SENDTO: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// "No rooms".
const EMPTY: u8 = 0x7E;

type SendTo = unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;

/// `sendto(socket, buffer, length, flags, address, address length)`: the lobby's port sent to
/// the payload's server.
unsafe extern "system" fn sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    let f: SendTo = unsafe { std::mem::transmute(ORIG_SENDTO.load(std::sync::atomic::Ordering::Relaxed)) };
    let port = PORT.load(std::sync::atomic::Ordering::Relaxed);
    // sockaddr_in: family (2 = AF_INET), port (big endian), address
    if !to.is_null() && tolen >= 16 && unsafe { *to } == 2 && u16::from_be_bytes(unsafe { [*to.add(2), *to.add(3)] }) == port {
        let mut local = [0u8; 16];
        unsafe { std::ptr::copy_nonoverlapping(to, local.as_mut_ptr(), 16) };
        local[4..8].copy_from_slice(&[127, 0, 0, 1]);
        return unsafe { f(s, buf, len, flags, local.as_ptr(), 16) };
    }
    unsafe { f(s, buf, len, flags, to, tolen) }
}

/// The answer to one query.
fn answer(query: &[u8]) -> Vec<u8> {
    let text: String = query.iter().map(|&b| if b == 0 { '|' } else { b as char }).collect();
    log!("lobby: query {text:?}");
    match query.first() {
        Some(b'2') => {
            let seq = text.split("seqNo\\").nth(1).map(|r| r.chars().take_while(char::is_ascii_digit).collect::<String>()).and_then(|d| d.parse::<u32>().ok()).unwrap_or(0);
            let mut out = seq.to_be_bytes().to_vec();
            out.extend([EMPTY, 0, 0]);
            out
        }
        _ => vec![EMPTY],
    }
}

fn serve(port: u16) {
    let socket = match UdpSocket::bind(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => return log!("lobby: 127.0.0.1:{port}: {e}"),
    };
    let mut buf = [0u8; 4096];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
        let _ = socket.send_to(&answer(&buf[..n]), from);
    }
}

pub(crate) fn init() {
    let Some(port) = std::env::var("WAL_NESICA_LOBBY").ok().and_then(|v| v.trim().parse::<u16>().ok()) else { return };
    let engine = unsafe { GetModuleHandleW(windows_sys::w!("engine.dll")) } as usize;
    if engine == 0 {
        return log!("lobby: engine.dll not loaded");
    }
    match unsafe { iat::hook_ordinal(engine, "WSOCK32.dll", 20, sendto as *const () as usize) } {
        Some(o) => ORIG_SENDTO.store(o, std::sync::atomic::Ordering::Relaxed),
        None => return log!("lobby: engine.dll does not import sendto"),
    }
    PORT.store(port, std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(move || serve(port));
    log!("lobby: port {port} answered (no rooms)");
}
