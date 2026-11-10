//! Initial D ARCADE STAGE Zero's game server (SDDF, v1.30; the players' data: Aime card
//! profiles, cars, story, time attack, teams): its "userdb", a TCP server the game reaches at
//! ALL.Net's startup host (the router's port 10000), its connections going to this server in its
//! own process, 127.0.0.1:`WAL_PORT`+107 (`allnet.rs`). Ported from minime v005 (`src/idz`,
//! shipped with the game's dump: the request / answer layouts and the profile logic) and
//! ARTEMiS (WTFPL, gitea.tendokyu.moe/Dniel97/artemis, `titles/idz`: the per-version codes,
//! the server list's news URLs, the serverbox magic); the players are kept in `Y:\idz\` (one
//! JSON file per Aime user id, `idz/store.rs`), the messages in `idz/msg.rs`. Ver.2 (header
//! protocol "210" / "230", v2.10 to v2.30) from minime v016 (github.com/djhackersdev/minime,
//! its `funcList210` codecs) and ARTEMiS's v2.30 codes, `idz/msg/v2.rs`, its players in
//! `Y:\idz2\`.
//!
//! The session: on connect the server sends 0x48 bytes, its AES-128 session key RSA-encrypted
//! with one of the keys the game ships (64 bytes little endian: key^e mod N), the u32
//! 0x01020304 and the key's hash (minime's fixed key pair and session key); then both ways are
//! AES-128-ECB with that key, no padding. A request is a 0x30-byte header (u32 magic
//! 0x01020304, the version in ASCII at 16: "130") and a payload of its code's length (its u16
//! code first); an answer is its payload only, a multiple of 16 bytes. The header may come alone
//! (ARTEMiS: in its own chunk) and the requests after it without one. A Ver.2 request whose
//! length is unknown or not sure (v2.30's own codes) is taken whole once the line has been quiet
//! for 50 ms (the game waits for the answer), every size logged.
//!
//! Also minime's UDP echo servers (its ping sockets: the router's ports 10001, 10003, 10010,
//! 10011; and 10009, which the game pings too) on 127.0.0.1:`WAL_PORT`+108.. ([`udp_echo_target`]), and the news text the game
//! fetches from the router's port 80 ([`news`]).

mod msg;
mod store;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::log;

fn base_port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT)
}

/// The userdb's local port (the game's connections to the router's port 10000).
/// The named event set once the game's configuration is answered (loadConfigB, 1.30's
/// loadConfig2): amdaemon's JVS board keeps the start button off until then ([`hold_start`]).
const READY_EVENT: &str = "WAL_IDZ_SERVER_READY";

/// The game has its game server's configuration: cards can be read.
pub(crate) fn mark_ready() {
    use windows_sys::Win32::System::Threading::CreateEventW;
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let name: Vec<u16> = READY_EVENT.encode_utf16().chain([0]).collect();
        // kept open for the game's life (its existence is the signal)
        let h = unsafe { CreateEventW(std::ptr::null(), 1, 1, name.as_ptr()) };
        log!("idz: configuration loaded, cards accepted{}", if h.is_null() { " (no event)" } else { "" });
    });
}

/// From another process (amdaemon): the game has its configuration.
pub(crate) fn is_ready() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};
    let name: Vec<u16> = READY_EVENT.encode_utf16().chain([0]).collect();
    let h = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
    if h.is_null() {
        return false;
    }
    unsafe { CloseHandle(h) };
    true
}

/// In amdaemon: the start button held off until the game has its game server's configuration.
/// It asks for it in steps 30 s apart from its attract (about 90 s after it shows), pauses them
/// once a player is in its entry screens, and a card read before it fails (error 6503,
/// "[accessStep] FAILED" in the game's log, the player plays without data).
pub(crate) fn hold_start() {
    fn hold(players: &mut [wal_payload_common::jvs_board::Switches]) {
        static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if READY.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if is_ready() {
            READY.store(true, std::sync::atomic::Ordering::Relaxed);
            log!("idz: start button on (the game's configuration loaded)");
            return;
        }
        for p in players.iter_mut() {
            p.0 &= !0x80;
        }
    }
    wal_payload_common::jvs_board::set_switches_hook(hold);
}

pub(crate) fn port() -> u16 {
    base_port() + 107
}

/// minime's echo ports on the router (its `idzPing`s).
const ECHO_PORTS: [u16; 5] = [10001, 10003, 10009, 10010, 10011];

/// A datagram to the router's `port`: the local echo socket's port, for the echo ports.
pub(crate) fn udp_echo_target(port: u16) -> Option<u16> {
    ECHO_PORTS.iter().position(|&p| p == port).map(|i| base_port() + 108 + i as u16)
}

/// A datagram from local `port`: the router's echo port it stands for (the answer's source).
pub(crate) fn udp_echo_origin(port: u16) -> Option<u16> {
    (0..ECHO_PORTS.len()).find(|&i| base_port() + 108 + i as u16 == port).map(|i| ECHO_PORTS[i])
}

/// The answer to the game's HTTP GET of `path` on the router's port 80, for
/// `/idz/news/<file>` (the server list's news URL, ARTEMiS's text: valid from 1979 to 2099;
/// to send as `text/plain; charset=utf-8`) and `/idz/error` (empty).
pub(crate) fn news(path: &str) -> Option<String> {
    if path.starts_with("/idz/news/") {
        Some("1979/01/01 00:00:00 2099/12/31 23:59:59 Welcome to Initial D Arcade Stage Zero on WindowsArcadeLoader!\r\n".to_string())
    } else if path.starts_with("/idz/error") {
        Some(String::new())
    } else {
        None
    }
}

// ── the handshake ─────────────────────────────────────────────────────────────────────

/// minime's key pair (`setup.ts`: one of the ten the game ships), decimal.
const RSA_N: &str = "4922323266120814292574970172377860734034664704992758249880018618131907367614177800329506877981986877921220485681998287752778495334541127048495486311792061";
const RSA_E: &str = "3961365081960959178294197133768419551060435043430437330799371731939550352626564261219865471710058480523874787120718634318364066605378505537556570049131337";
const RSA_HASH_N: u32 = 2662304617;

/// minime's session key, 0xffddeeccbbaa99887766554433221100 little endian.
const SESSION_KEY: [u8; 16] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xee, 0xdd, 0xff];

/// Magic numbers of the header and of a serverbox request (ARTEMiS: ignored).
const MAGIC: u32 = 0x0102_0304;
const SERVERBOX: u32 = 0xfe78_571d;
const HEADER: usize = 0x30;

/// A number as 32-bit limbs, lowest first.
fn decimal(text: &str) -> Vec<u32> {
    let mut n = vec![0u32];
    for d in text.bytes().map(|c| (c - b'0') as u64) {
        let mut carry = d;
        for l in n.iter_mut() {
            let v = *l as u64 * 10 + carry;
            *l = v as u32;
            carry = v >> 32;
        }
        if carry != 0 {
            n.push(carry as u32);
        }
    }
    n
}

/// `a * b mod m` (schoolbook product, bitwise reduction).
fn mul_mod(a: &[u32], b: &[u32], m: &[u32]) -> Vec<u32> {
    let mut prod = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            let v = prod[i + j] as u64 + x as u64 * y as u64 + carry;
            prod[i + j] = v as u32;
            carry = v >> 32;
        }
        prod[i + b.len()] = carry as u32;
    }
    let mut r = vec![0u32; m.len() + 1];
    for bit in (0..prod.len() * 32).rev() {
        // r = r * 2 + bit
        let mut carry = prod[bit / 32] >> (bit % 32) & 1;
        for l in r.iter_mut() {
            let next = *l >> 31;
            *l = *l << 1 | carry;
            carry = next;
        }
        // r >= m: r -= m
        let ge = (0..r.len()).rev().find_map(|i| {
            let mi = m.get(i).copied().unwrap_or(0);
            (r[i] != mi).then_some(r[i] > mi)
        });
        if ge.unwrap_or(true) {
            let mut borrow = 0i64;
            for (i, l) in r.iter_mut().enumerate() {
                let v = *l as i64 - m.get(i).copied().unwrap_or(0) as i64 - borrow;
                borrow = i64::from(v < 0);
                *l = v.rem_euclid(1 << 32) as u32;
            }
        }
    }
    r.truncate(m.len());
    r
}

/// `base^exp mod m`.
fn mod_pow(base: &[u32], exp: &[u32], m: &[u32]) -> Vec<u32> {
    let mut acc = vec![0u32; m.len()];
    acc[0] = 1;
    for bit in (0..exp.len() * 32).rev() {
        acc = mul_mod(&acc, &acc, m);
        if exp[bit / 32] >> (bit % 32) & 1 == 1 {
            acc = mul_mod(&acc, base, m);
        }
    }
    acc
}

/// The setup message (0x48 bytes), computed once.
fn handshake() -> &'static [u8; 0x48] {
    static MSG: OnceLock<[u8; 0x48]> = OnceLock::new();
    MSG.get_or_init(|| {
        let key: Vec<u32> = SESSION_KEY.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let enc = mod_pow(&key, &decimal(RSA_E), &decimal(RSA_N));
        let mut msg = [0u8; 0x48];
        for (i, l) in enc.iter().take(16).enumerate() {
            msg[4 * i..4 * i + 4].copy_from_slice(&l.to_le_bytes());
        }
        msg[0x40..0x44].copy_from_slice(&MAGIC.to_le_bytes());
        msg[0x44..0x48].copy_from_slice(&RSA_HASH_N.to_le_bytes());
        msg
    })
}

// ── the stream ────────────────────────────────────────────────────────────────────────

/// What the request stream gave.
#[derive(Debug, PartialEq)]
enum Frame {
    /// A request: the header's version, the payload (its code first).
    Request(String, Vec<u8>),
    /// A Ver.1 code without a known length: the buffered data dropped.
    Unknown(u16),
    /// A serverbox request (ARTEMiS answers one byte and skips the next data).
    ServerBox,
    /// Data without the header's magic, dropped (its first decrypted bytes kept for the log).
    BadMagic(u32, Vec<u8>),
}

/// How long the line stays quiet before a Ver.2 request of uncertain length is taken whole (the
/// game sends a request and waits for its answer).
const IDLE: std::time::Duration = std::time::Duration::from_millis(50);

/// The request stream: decrypted in 16-byte blocks as they arrive, cut into requests. The
/// header comes before the requests (minime: once per connection, the requests after it;
/// ARTEMiS: its own chunk, the request in the next one at offset 0): it is taken alone and
/// its version applies to the requests after it.
struct Framer {
    aes: Aes128,
    /// Received bytes short of a block.
    raw: Vec<u8>,
    /// Decrypted bytes not yet a whole request.
    buf: Vec<u8>,
    /// ARTEMiS's skip after a serverbox request: the next received data ignored.
    skip_next: bool,
    /// The last header's version.
    version: Option<String>,
}

impl Framer {
    fn new() -> Framer {
        Framer { aes: Aes128::new(GenericArray::from_slice(&SESSION_KEY)), raw: Vec::new(), buf: Vec::new(), skip_next: false, version: None }
    }

    /// Received data; false when it was skipped (after a serverbox request).
    fn push(&mut self, data: &[u8]) -> bool {
        if self.skip_next {
            self.skip_next = false;
            return false;
        }
        self.raw.extend_from_slice(data);
        let whole = self.raw.len() / 16 * 16;
        for block in self.raw[..whole].chunks(16) {
            let mut b = GenericArray::clone_from_slice(block);
            self.aes.decrypt_block(&mut b);
            self.buf.extend_from_slice(&b);
        }
        self.raw.drain(..whole);
        true
    }

    fn proto(&self) -> msg::Proto {
        msg::Proto::of(self.version.as_deref().unwrap_or(""))
    }

    /// Whether the client should wait [`IDLE`] for more data, then call `next(true)` (a Ver.2
    /// request not yet taken).
    fn pending(&self) -> bool {
        self.version.is_some() && self.proto().v2() && self.buf.len() >= 2 && r32_magic(&self.buf) != Some(MAGIC)
    }

    /// The next frame; `idle`: the line has been quiet for [`IDLE`] (a Ver.2 request of
    /// uncertain or unknown length, or one shorter than expected, taken whole).
    fn next(&mut self, idle: bool) -> Option<Frame> {
        loop {
            if self.buf.len() < 4 {
                return None;
            }
            let magic = msg::r32(&self.buf, 0);
            if magic == SERVERBOX {
                self.buf.clear();
                self.raw.clear();
                self.skip_next = true;
                return Some(Frame::ServerBox);
            }
            if magic == MAGIC {
                if self.buf.len() < HEADER {
                    return None;
                }
                self.version = Some(String::from_utf8_lossy(&self.buf[16..19]).trim_end_matches('\0').to_string());
                self.buf.drain(..HEADER);
                continue;
            }
            let Some(version) = self.version.clone() else {
                let head = self.buf[..self.buf.len().min(64)].to_vec();
                self.buf.clear();
                return Some(Frame::BadMagic(magic, head));
            };
            let proto = self.proto();
            let code = msg::r16(&self.buf, 0) as u16;
            if !proto.v2() {
                let Some((len, _)) = msg::request_info(proto, code) else {
                    self.buf.clear();
                    return Some(Frame::Unknown(code));
                };
                if self.buf.len() < len {
                    return None;
                }
                let payload = self.buf.drain(..len).collect();
                return Some(Frame::Request(version, payload));
            }
            // Ver.2: a sure length taken when it is all there (and what follows looks like the
            // next request), else everything once the line is quiet
            let take = match msg::request_info(proto, code) {
                Some((len, true)) if self.buf.len() == len || (self.buf.len() > len && plausible(proto, &self.buf[len..])) => len,
                _ if idle => self.buf.len(),
                _ => return None,
            };
            let payload = self.buf.drain(..take).collect();
            return Some(Frame::Request(version, payload));
        }
    }
}

/// The first u32 of `data`, when it has one.
fn r32_magic(data: &[u8]) -> Option<u32> {
    (data.len() >= 4).then(|| msg::r32(data, 0))
}

/// Whether `rest` starts like a frame (a header, a serverbox request, a known code).
fn plausible(proto: msg::Proto, rest: &[u8]) -> bool {
    rest.len() >= 4 && {
        let magic = msg::r32(rest, 0);
        magic == MAGIC || magic == SERVERBOX || msg::request_info(proto, msg::r16(rest, 0) as u16).is_some()
    }
}

/// An answer encrypted (zero padded to the cipher's blocks).
fn seal(aes: &Aes128, mut data: Vec<u8>) -> Vec<u8> {
    data.resize(data.len().div_ceil(16) * 16, 0);
    for block in data.chunks_mut(16) {
        aes.encrypt_block(GenericArray::from_mut_slice(block));
    }
    data
}

/// The frames of `framer` answered.
fn answer(stream: &mut TcpStream, framer: &mut Framer, version: &mut String, first: &[u8], idle: bool, bad_logged: &mut bool) -> std::io::Result<()> {
    while let Some(frame) = framer.next(idle) {
        match frame {
            Frame::Request(v, payload) => {
                if v != *version {
                    log!("idz: game version {v}");
                    *version = v.clone();
                }
                let proto = msg::Proto::of(&v);
                let code = msg::r16(&payload, 0) as u16;
                let info = msg::request_info(proto, code);
                match info {
                    None => log!("idz: unknown code {code:#06x} ({} bytes)", payload.len()),
                    Some((len, sure)) if !sure || len != payload.len() => {
                        log!("idz: {code:#06x} {}: {:#x} bytes (expected {len:#x}{})", msg::name(proto, code), payload.len(), if sure { "" } else { ", a guess" })
                    }
                    _ => {}
                }
                if std::env::var("WAL_ALLS_IDZ_RAW").is_ok_and(|v| v.trim() == "1") {
                    // debug: the request's first bytes
                    let hex: String = payload.iter().take(0x40).map(|b| format!("{b:02x}")).collect();
                    log!("idz: {code:#06x} raw {hex}");
                    let _ = std::fs::write(format!("Y:\\idz_raw_{code:04x}.bin"), &payload);
                }
                let answer = msg::handle(proto, code, &payload);
                log!(
                    "idz: {code:#06x} {} {:#x} bytes (aime {}) -> {:#06x}, {:#x} bytes",
                    msg::name(proto, code),
                    payload.len(),
                    msg::r32(&payload, 4),
                    msg::r16(&answer, 0),
                    answer.len()
                );
                stream.write_all(&seal(&framer.aes, answer))?;
            }
            Frame::Unknown(code) => {
                // ARTEMiS's generic answer
                log!("idz: unknown code {code:#06x}, its data dropped");
                stream.write_all(&seal(&framer.aes, msg::Out::new(0x30, 0x0001).done()))?;
            }
            Frame::ServerBox => {
                log!("idz: serverbox request");
                stream.write_all(&[0])?;
            }
            Frame::BadMagic(magic, head) => {
                // ARTEMiS answers a byte to data before any header
                log!("idz: bad magic {magic:#010x}, data dropped");
                stream.write_all(&[0])?;
                if !*bad_logged {
                    *bad_logged = true;
                    log!("idz: raw {}", store::hex(&first[..first.len().min(64)]));
                    log!("idz: decrypted {}", store::hex(&head));
                }
            }
        }
    }
    Ok(())
}

fn client(mut stream: TcpStream) -> std::io::Result<()> {
    stream.write_all(handshake())?;
    let mut framer = Framer::new();
    let mut version = String::new();
    let mut chunk = [0u8; 8192];
    // the connection's first received bytes (the log of a bad magic)
    let mut first: Vec<u8> = Vec::new();
    let mut bad_logged = false;
    loop {
        stream.set_read_timeout(if framer.pending() { Some(IDLE) } else { None })?;
        let idle = match stream.read(&mut chunk) {
            Ok(0) => {
                // what is left, answered if the game still reads
                let _ = answer(&mut stream, &mut framer, &mut version, &first, true, &mut bad_logged);
                return Ok(());
            }
            Ok(n) => {
                if first.len() < 64 {
                    first.extend_from_slice(&chunk[..n.min(64 - first.len())]);
                }
                if !framer.push(&chunk[..n]) {
                    stream.write_all(&[0])?;
                    continue;
                }
                false
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => true,
            Err(e) => return Err(e),
        };
        answer(&mut stream, &mut framer, &mut version, &first, idle, &mut bad_logged)?;
    }
}

fn serve() {
    let port = port();
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            log!("idz: userdb on 127.0.0.1:{port}");
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    if let Err(e) = client(stream) {
                        log!("idz: connection: {e}");
                    }
                });
            }
        }
        Err(e) => log!("idz: 127.0.0.1:{port}: {e}"),
    }
}

/// The echo sockets: each datagram sent back.
fn echo(port: u16) {
    match std::net::UdpSocket::bind(("127.0.0.1", port)) {
        Ok(socket) => {
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = socket.recv_from(&mut buf) {
                let _ = socket.send_to(&buf[..n], from);
            }
        }
        Err(e) => log!("idz: echo port {port}: {e}"),
    }
}

pub(crate) fn init() {
    let _ = std::fs::create_dir_all(store::DIR);
    let _ = std::fs::create_dir_all(store::DIR2);
    // the handshake's RSA computed before the game connects
    std::thread::spawn(|| {
        handshake();
        serve();
    });
    for port in ECHO_PORTS {
        if let Some(local) = udp_echo_target(port) {
            std::thread::spawn(move || echo(local));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_matches_minime() {
        // pow(0xffddeeccbbaa99887766554433221100, e, N) little endian (minime's setup.ts)
        let expected = "69f39b35089124a25fc59f040cc29d15da98e011714955391a9fb8ad84095ed61ec174fa1909efa0a7a21439105727a700954fc21fd30413aee6bbf5c68be720";
        let msg = handshake();
        assert_eq!(store::hex(&msg[..0x40]), expected);
        assert_eq!(msg::r32(msg, 0x40), MAGIC);
        assert_eq!(msg::r32(msg, 0x44), 2662304617);
    }

    #[test]
    fn hash_of_n() {
        // ARTEMiS's rsaHashKeyN of the key's decimal N
        const LUT: [u32; 16] = [
            0x9C82E674, 0x5A4738D9, 0x8B8D7AE0, 0x29EC9D81, 0x1209091B, 0x1D83839E, 0x582C2C74, 0x341A1A2E, 0x361B1B2D, 0xDC6E6EB2, 0xB45A5AEE, 0x5BA0A0FB, 0xA45252F6,
            0x763B3B4D, 0xB7D6D661, 0x7DB3B3CE,
        ];
        let hash = RSA_N.bytes().fold(0u32, |h, c| h.wrapping_mul(5).wrapping_add((c ^ 0xb3) as u32) ^ LUT[(c & 0xf) as usize]);
        assert_eq!(hash, RSA_HASH_N);
    }

    fn request(code: u16, len: usize) -> Vec<u8> {
        request_as(b"130", code, len)
    }

    fn request_as(version: &[u8; 3], code: u16, len: usize) -> Vec<u8> {
        let mut r = vec![0u8; HEADER + len];
        r[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        r[16..19].copy_from_slice(version);
        if len >= 2 {
            r[HEADER..HEADER + 2].copy_from_slice(&code.to_le_bytes());
        }
        r
    }

    /// A request without its header.
    fn bare(code: u16, len: usize) -> Vec<u8> {
        let mut r = vec![0u8; len];
        r[0..2].copy_from_slice(&code.to_le_bytes());
        r
    }

    #[test]
    fn framing_230() {
        let aes = Aes128::new(GenericArray::from_slice(&SESSION_KEY));
        let mut f = Framer::new();
        // minime's way: header and lockProfile2 in one stream, a sure length: taken at once
        f.push(&seal(&aes, request_as(b"230", 0x0065, 0x20)));
        assert!(matches!(f.next(false), Some(Frame::Request(v, p)) if v == "230" && p.len() == 0x20));
        assert_eq!(f.next(false), None);
        assert!(!f.pending());
        // ARTEMiS's way: the header alone, the request in the next chunk
        f.push(&seal(&aes, request_as(b"230", 0, 0)));
        assert_eq!(f.next(false), None);
        f.push(&seal(&aes, bare(0x0067, 0x10)));
        assert!(matches!(f.next(false), Some(Frame::Request(v, p)) if v == "230" && p.len() == 0x10 && msg::r16(&p, 0) == 0x0067));
        // an unknown code: kept until the line is quiet, then taken whole
        f.push(&seal(&aes, bare(0x7777, 0x40)));
        assert_eq!(f.next(false), None);
        assert!(f.pending());
        assert!(matches!(f.next(true), Some(Frame::Request(_, p)) if p.len() == 0x40 && msg::r16(&p, 0) == 0x7777));
        assert!(!f.pending());
        // saveProfile 0x0143, its length a guess: taken whole once quiet, longer than the guess
        let save = request_as(b"230", 0x0143, 0x1000);
        for piece in seal(&aes, save).chunks(0x400) {
            f.push(piece);
            assert_eq!(f.next(false), None);
        }
        assert!(matches!(f.next(true), Some(Frame::Request(_, p)) if p.len() == 0x1000));
        // a sure length shorter than sent, its rest not a request: taken whole once quiet
        f.push(&seal(&aes, bare(0x0067, 0x20)));
        assert_eq!(f.next(false), None);
        assert!(matches!(f.next(true), Some(Frame::Request(_, p)) if p.len() == 0x20));
        // two pipelined requests: cut at the sure length
        let mut two = bare(0x0067, 0x10);
        two.extend(bare(0x0065, 0x20));
        f.push(&seal(&aes, two));
        assert!(matches!(f.next(false), Some(Frame::Request(_, p)) if p.len() == 0x10));
        assert!(matches!(f.next(false), Some(Frame::Request(_, p)) if p.len() == 0x20));
        assert_eq!(msg::Proto::of("230"), msg::Proto::V230);
        assert_eq!(msg::Proto::of("210"), msg::Proto::V210);
        assert_eq!(msg::Proto::of("130"), msg::Proto::V1);
    }

    #[test]
    fn bad_magic_before_header() {
        let aes = Aes128::new(GenericArray::from_slice(&SESSION_KEY));
        let mut f = Framer::new();
        f.push(&seal(&aes, bare(0x0067, 0x10)));
        assert!(matches!(f.next(false), Some(Frame::BadMagic(0x0000_0067, head)) if head.len() == 0x10));
        // a serverbox request: the next chunk skipped
        let mut sb = vec![0u8; 16];
        sb[0..4].copy_from_slice(&SERVERBOX.to_le_bytes());
        f.push(&seal(&aes, sb));
        assert_eq!(f.next(false), Some(Frame::ServerBox));
        assert!(!f.push(&[0; 16]));
    }

    #[test]
    fn framing_in_pieces() {
        let aes = Aes128::new(GenericArray::from_slice(&SESSION_KEY));
        let mut data = request(0x0006, 0x20);
        data.extend(request(0x012f, 0x20));
        let enc = seal(&aes, data);
        let mut f = Framer::new();
        let mut frames = Vec::new();
        // odd sized pieces
        for piece in enc.chunks(7) {
            assert!(f.push(piece));
            while let Some(frame) = f.next(false) {
                frames.push(frame);
            }
        }
        assert_eq!(frames.len(), 2);
        match &frames[1] {
            Frame::Request(v, p) => {
                assert_eq!(v, "130");
                assert_eq!(p.len(), 0x20);
                assert_eq!(msg::r16(p, 0), 0x012f);
            }
            other => panic!("{other:?}"),
        }
        // an unknown code: dropped, the stream goes on
        f.push(&seal(&aes, request(0x7777, 0x10)));
        assert_eq!(f.next(false), Some(Frame::Unknown(0x7777)));
        f.push(&seal(&aes, request(0x0004, 0x50)));
        assert!(matches!(f.next(false), Some(Frame::Request(..))));
        assert_eq!(f.next(false), None);
    }

    #[test]
    fn aes_round_trip() {
        let aes = Aes128::new(GenericArray::from_slice(&SESSION_KEY));
        let plain: Vec<u8> = (0..40u8).collect();
        let enc = seal(&aes, plain.clone());
        assert_eq!(enc.len(), 48);
        let mut f = Framer::new();
        f.push(&enc);
        assert_eq!(&f.buf[..40], &plain[..]);
    }

    #[test]
    fn echo_ports() {
        let local = udp_echo_target(10010).unwrap();
        assert_eq!(udp_echo_origin(local), Some(10010));
        assert_eq!(udp_echo_target(10000), None);
    }
}
