//! The game's datagrams on the emulated network (`netenv.rs`: one adapter at .205 of the
//! keychip's subnet, absent from the machine). Sega World Drivers Championship's main cabinet
//! announces itself to the subnet's broadcast address (port 40257) and must hear its own
//! announcement (else no main cabinet is found: error 8404 "LAN install server not found", the
//! game's own). Datagrams sent to the subnet go to the loopback address (its broadcast address
//! to lo's: every socket bound to the port gets them) and come back from the adapter's address;
//! a socket bound to the adapter's address is bound to every address.
//!
//! Initial D THE ARCADE ([`init_echo`]): only its echo servers (the game server's NAT check,
//! ARTEMiS's `echo.py`): datagrams to the router's ports 20001 / 20002 go to echo sockets on
//! 127.0.0.1:`WAL_PORT`+105 / +106 and come back from the router's port; Initial D ARCADE STAGE
//! Zero's (minime's ping ports 10001, 10003, 10010, 10011: `idz.rs`) the same way.

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

static BIND: AtomicUsize = AtomicUsize::new(0);
static SEND_TO: AtomicUsize = AtomicUsize::new(0);
static RECV_FROM: AtomicUsize = AtomicUsize::new(0);
static TRACES: AtomicUsize = AtomicUsize::new(0);
/// Only the echo servers redirected ([`init_echo`]).
static ECHO_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The game server's echo ports on the router.
const ECHO_PORTS: [u16; 2] = [20001, 20002];

fn echo_port(i: usize) -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 105 + i as u16
}

/// A datagram to the router's echo port: the local echo socket's address.
fn echo_target(address: *const u8, length: i32) -> Option<[u8; 16]> {
    if address.is_null() || length < 16 {
        return None;
    }
    let a = unsafe { std::slice::from_raw_parts(address, 16) };
    let port = u16::from_be_bytes([a[2], a[3]]);
    // Initial D ARCADE STAGE Zero's echo ports (`idz.rs`)
    let local = match ECHO_PORTS.iter().position(|&p| p == port) {
        Some(i) => echo_port(i),
        None => crate::idz::udp_echo_target(port)?,
    };
    if a[0] != 2 || a[4..8] != crate::netenv::router() {
        return None;
    }
    let mut m = [0u8; 16];
    m.copy_from_slice(a);
    m[2..4].copy_from_slice(&local.to_be_bytes());
    m[4..8].copy_from_slice(&[127, 0, 0, 1]);
    Some(m)
}

/// The redirections in the log, each kind once (the first 32).
fn trace(args: std::fmt::Arguments) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let text = args.to_string();
    let mut seen = SEEN.lock().unwrap();
    if !seen.contains(&text) && TRACES.fetch_add(1, Ordering::Relaxed) < 32 {
        log!("udp: {text}");
        seen.push(text);
    }
}

/// An IPv4 `sockaddr_in` (16 bytes) with its address replaced when `map` gives one.
fn mapped(address: *const u8, length: i32, map: impl Fn([u8; 4]) -> Option<[u8; 4]>) -> Option<[u8; 16]> {
    if address.is_null() || length < 16 {
        return None;
    }
    let a = unsafe { std::slice::from_raw_parts(address, 16) };
    if a[0] != 2 {
        return None;
    }
    let to = map([a[4], a[5], a[6], a[7]])?;
    let mut m = [0u8; 16];
    m.copy_from_slice(a);
    m[4..8].copy_from_slice(&to);
    Some(m)
}

fn bound(addr: [u8; 4]) -> Option<[u8; 4]> {
    (addr == crate::netenv::iface()).then_some([0, 0, 0, 0])
}

fn sent(addr: [u8; 4]) -> Option<[u8; 4]> {
    let subnet = crate::netenv::iface();
    if addr == [255; 4] || (addr[..3] == subnet[..3] && addr[3] == 255) {
        return Some([127, 255, 255, 255]);
    }
    (addr[..3] == subnet[..3]).then_some([127, 0, 0, 1])
}

unsafe extern "system" fn bind(socket: usize, address: *const u8, length: i32) -> i32 {
    let real: unsafe extern "system" fn(usize, *const u8, i32) -> i32 = unsafe { std::mem::transmute(BIND.load(Ordering::Relaxed)) };
    if ECHO_ONLY.load(Ordering::Relaxed) {
        return unsafe { real(socket, address, length) };
    }
    match mapped(address, length, bound) {
        Some(m) => {
            trace(format_args!("bind port {} on every address", u16::from_be_bytes([m[2], m[3]])));
            unsafe { real(socket, m.as_ptr(), length) }
        }
        None => unsafe { real(socket, address, length) },
    }
}

unsafe extern "system" fn send_to(socket: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, to_len: i32) -> i32 {
    type F = unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32;
    let real: F = unsafe { std::mem::transmute(SEND_TO.load(Ordering::Relaxed)) };
    if let Some(m) = echo_target(to, to_len) {
        return unsafe { real(socket, buf, len, flags, m.as_ptr(), to_len) };
    }
    if ECHO_ONLY.load(Ordering::Relaxed) {
        return unsafe { real(socket, buf, len, flags, to, to_len) };
    }
    match mapped(to, to_len, sent) {
        Some(m) => {
            trace(format_args!("sendto {}.{}.{}.{} port {}: loopback", unsafe { *to.add(4) }, unsafe { *to.add(5) }, unsafe { *to.add(6) }, unsafe { *to.add(7) }, u16::from_be_bytes([m[2], m[3]])));
            unsafe { real(socket, buf, len, flags, m.as_ptr(), to_len) }
        }
        None => unsafe { real(socket, buf, len, flags, to, to_len) },
    }
}

unsafe extern "system" fn recv_from(socket: usize, buf: *mut u8, len: i32, flags: i32, from: *mut u8, from_len: *mut i32) -> i32 {
    type F = unsafe extern "system" fn(usize, *mut u8, i32, i32, *mut u8, *mut i32) -> i32;
    let real: F = unsafe { std::mem::transmute(RECV_FROM.load(Ordering::Relaxed)) };
    let n = unsafe { real(socket, buf, len, flags, from, from_len) };
    // a datagram from the loopback address: one sent to the subnet, from the adapter's address
    if n >= 0 && !from.is_null() && !from_len.is_null() && unsafe { *from_len } >= 8 {
        let a = unsafe { std::slice::from_raw_parts_mut(from, 8) };
        let port = u16::from_be_bytes([a[2], a[3]]);
        if a[0] == 2 && a[4] == 127 {
            // an echo server's answer: from the router's echo port
            if let Some(i) = (0..ECHO_PORTS.len()).find(|&i| echo_port(i) == port) {
                a[2..4].copy_from_slice(&ECHO_PORTS[i].to_be_bytes());
                a[4..8].copy_from_slice(&crate::netenv::router());
            } else if let Some(origin) = crate::idz::udp_echo_origin(port) {
                a[2..4].copy_from_slice(&origin.to_be_bytes());
                a[4..8].copy_from_slice(&crate::netenv::router());
            } else if !ECHO_ONLY.load(Ordering::Relaxed) {
                a[4..8].copy_from_slice(&crate::netenv::iface());
            }
        }
    }
    n
}

pub(crate) fn init() {
    let exe = iat::game_module();
    let hooks: [(u16, &str, usize, &AtomicUsize); 3] = [
        (2, "bind", bind as *const () as usize, &BIND),
        (17, "recvfrom", recv_from as *const () as usize, &RECV_FROM),
        (20, "sendto", send_to as *const () as usize, &SEND_TO),
    ];
    for (ordinal, name, f, slot) in hooks {
        // imported by ordinal, or by name (Initial D THE ARCADE)
        let orig = unsafe { iat::hook_ordinal(exe, "WS2_32.dll", ordinal, f) }.or_else(|| unsafe { iat::hook("ws2_32.dll", name, f) });
        match orig {
            Some(orig) => slot.store(orig, Ordering::Relaxed),
            None => log!("udp: WS2_32 {name} not imported"),
        }
    }
}

/// The game server's echo servers only (Initial D THE ARCADE's NAT check): each datagram sent
/// back.
pub(crate) fn init_echo() {
    ECHO_ONLY.store(true, Ordering::Relaxed);
    init();
    serve_echo();
}

/// The subnet's datagrams and the game server's echo servers (Initial D ARCADE STAGE Zero: its
/// store server found by a broadcast to the subnet, its game server's echo ports).
pub(crate) fn init_with_echo() {
    init();
    serve_echo();
}

fn serve_echo() {
    for i in 0..ECHO_PORTS.len() {
        std::thread::spawn(move || match std::net::UdpSocket::bind(("127.0.0.1", echo_port(i))) {
            Ok(socket) => {
                let mut buf = [0u8; 2048];
                while let Ok((n, from)) = socket.recv_from(&mut buf) {
                    let _ = socket.send_to(&buf[..n], from);
                }
            }
            Err(e) => log!("udp: echo port {}: {e}", echo_port(i)),
        });
    }
}
