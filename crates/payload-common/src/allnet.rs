//! ALL.Net, Sega's authentication server a cabinet reaches at boot
//! (`http://naominet.jp/sys/servlet/PowerOn`): answered by the payload so the power-on test
//! passes at once (formats from ARTEMiS' `core/allnet.py`, WTFPL). ALLS' amdaemon (WinHTTP) and
//! the games with their own ALL.Net client (Project DIVA Arcade's `alw`, raw sockets).
//!
//! The connections to an ALL.Net host (port 80 of the virtual router, the address
//! [`crate::dns`] gives these hosts; `WinHttpConnect`) go to the payload's server on
//! 127.0.0.1:`WAL_PORT`+102 ([`redirect_connect`], [`hook_winhttp`]). A request's body is a form
//! (`key=value&...`), packed when its `Pragma` header is `DFI` (zlib, then base64; the answer
//! packed the same way):
//! - `PowerOn`: refused (`stat=-3`, the game plays offline, at once); with
//!   `WAL_ALLS_ALLNET=1` accepted (`stat=1`, the shop's values, the title server's URI,
//!   `WAL_ALLS_TITLE_URI` / `WAL_ALLS_TITLE_HOST`: `{router}` the virtual router, `{gs}` the
//!   payload's game server, [`set_game_server`], `{allnet}` this server)
//! - `DownloadOrder`: no update (`uri=null`)
//!
//! A title server in this process ([`set_title`]: CHUNITHM's, a game with no amdaemon): the
//! requests outside `/sys/servlet/` are its, the game reaching the title uri's host (the router)
//! through WinHTTP too.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{dns, iat, log};

pub fn port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 102
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for &c in text {
        let Some(v) = B64.iter().position(|&b| b == c) else { continue };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// The form's values (`key=value&...`, first line).
fn form(text: &str) -> Vec<(String, String)> {
    text.lines().next().unwrap_or("").split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn value<'a>(form: &'a [(String, String)], key: &str) -> &'a str {
    form.iter().find(|(k, _)| k == key).map_or("", |(_, v)| v.as_str())
}

/// UTC now as (year, month, day, hour, minute, second).
fn utc_now() -> (i64, u32, u32, u32, u32, u32) {
    let secs = crate::clock::unix_now() as i64;
    let (days, rest) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // days to civil date (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + (month <= 2) as i64;
    (year, month, day, (rest / 3600) as u32, (rest / 60 % 60) as u32, (rest % 60) as u32)
}

/// Now as an HTTP date (`Sun, 08 Nov 2026 10:00:00 GMT`): the server's clock amdaemon may
/// take.
fn http_date() -> String {
    let (y, mo, d, h, mi, s) = utc_now();
    // day of the week: Zeller-like, from the days since 1970-01-01 (a Thursday)
    let secs = crate::clock::unix_now();
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(secs / 86400 % 7) as usize];
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][mo as usize - 1];
    format!("{weekday}, {d:02} {month} {y} {h:02}:{mi:02}:{s:02} GMT")
}

static GAME_SERVER: OnceLock<String> = OnceLock::new();

/// The payload's game server's address (`host:port`), `{gs}` in `WAL_ALLS_TITLE_URI`.
pub fn set_game_server(address: String) {
    let _ = GAME_SERVER.set(address);
}

fn power_on(req: &[(String, String)]) -> String {
    let format = value(req, "format_ver");
    let host = dns::dotted(dns::router());
    // the title server's URI (`WAL_ALLS_TITLE_URI`: an ABAAS game's GS server name, a bare host;
    // `{gs}` the payload's game server itself: Chrono Regalia's Mono sockets, not in the game
    // module's imports, never reach the router's redirection); `{allnet}` this server itself
    // (O.N.G.E.K.I.'s title server, in amdaemon's ALL.Net server, reached from its Mono)
    let gs = GAME_SERVER.get().cloned().unwrap_or_default();
    let own = format!("127.0.0.1:{}", port());
    let uri = std::env::var("WAL_ALLS_TITLE_URI").ok().filter(|u| !u.trim().is_empty()).map(|u| u.replace("{router}", &host).replace("{gs}", &gs).replace("{allnet}", &own)).unwrap_or_else(|| format!("http://{host}/{}/", value(req, "game_id")));
    // the title server's host (`WAL_ALLS_TITLE_HOST`: Initial D THE ARCADE's game server URL,
    // `<host>initiald/...`, https:// when it has no scheme; Initial D ARCADE STAGE Zero's
    // `{router}:10000`, its game server's address)
    let title_host = std::env::var("WAL_ALLS_TITLE_HOST").map_or(host.clone(), |h| h.trim().replace("{router}", &host));
    let mut resp = format!("stat=1&uri={}&host={title_host}&place_id=0123", uri.trim());
    resp += "&name=WindowsArcadeLoader&nickname=WAL&region0=1&region_name0=W&region_name1=X&region_name2=Y&region_name3=Z&setting=1";
    let (y, mo, d, h, mi, s) = utc_now();
    if format.starts_with('3') {
        resp += &format!("&country=JPN&allnet_id=123&client_timezone=+0900&utc_time={y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z&res_ver=3&token={}", value(req, "token"));
    } else {
        // the time in Japan
        let jst = utc_now_plus(9);
        resp += &format!("&year={}&month={}&day={}&hour={}&minute={}&second={}", jst.0, jst.1, jst.2, jst.3, jst.4, jst.5);
        if format.starts_with('2') {
            resp += "&country=JPN&timezone=+09:00&res_class=PowerOnResponseV2";
        }
    }
    resp + "\n"
}

fn utc_now_plus(hours: u32) -> (i64, u32, u32, u32, u32, u32) {
    // close to midnight the day is UTC's (only shown in the test menu)
    let (y, mo, d, h, mi, s) = utc_now();
    let h2 = h + hours;
    if h2 < 24 { (y, mo, d, h2, mi, s) } else { (y, mo, d, h, mi, s) }
}

fn handle(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let header = |name: &str| {
        head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()))
    };
    let length: usize = header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while data.len() < header_end + length {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    let body = &data[header_end..(header_end + length).min(data.len())];
    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
    if TITLE.get().is_none() && !path.contains("/sys/servlet/") {
        log!("allnet: {path}: no title server ({} bytes: {}...)", body.len(), body.iter().take(48).map(|b| format!("{b:02x}")).collect::<String>());
    }
    if let Some(title) = TITLE.get().filter(|_| !path.contains("/sys/servlet/")) {
        if std::env::var("WAL_ALLNET_LOG_HEAD").is_ok_and(|v| v.trim() == "1") {
            log!("allnet: {}", head.trim().replace("\r\n", " | "));
        }
        // a title server packing its bodies with the `x-rd2-*` headers (`set_title_rd2`): zlib
        // both ways
        let rd2 = TITLE_RD2.load(Ordering::Relaxed) && header("x-rd2-uncomp-len").is_some();
        let mut body = if rd2 {
            title(&path, &miniz_oxide::inflate::decompress_to_vec_zlib(body).unwrap_or_default())
        } else {
            title(&path, body)
        };
        // a title server answering as ALL.Net does (`set_title_dfi`): deflated then base64,
        // `Pragma: DFI`, when the request was
        let dfi = TITLE_DFI.load(Ordering::Relaxed) && header("Pragma").is_some_and(|v| v.eq_ignore_ascii_case("DFI"));
        if dfi {
            body = format!("{}\r\n", base64_encode(&miniz_oxide::deflate::compress_to_vec_zlib(&body, 6))).into_bytes();
        }
        let mut pragma = if dfi { "Pragma: DFI\r\n".to_string() } else { String::new() };
        if rd2 {
            let packed = miniz_oxide::deflate::compress_to_vec_zlib(&body, 6);
            pragma = format!(
                "Pragma: DFI\r\nx-rd2-comp-crc: {}\r\nx-rd2-comp-len: {}\r\nx-rd2-uncomp-crc: {}\r\nx-rd2-uncomp-len: {}\r\n",
                crc32(&packed),
                packed.len(),
                crc32(&body),
                body.len()
            );
            body = packed;
        }
        let mut r = format!("HTTP/1.1 200 OK\r\nDate: {}\r\nContent-Type: application/octet-stream\r\n{pragma}Content-Length: {}\r\nConnection: close\r\n\r\n", http_date(), body.len()).into_bytes();
        r.extend_from_slice(&body);
        return stream.write_all(&r);
    }
    let dfi = header("Pragma").is_some_and(|v| v.eq_ignore_ascii_case("DFI"));
    let text = if dfi {
        let packed = base64_decode(body);
        miniz_oxide::inflate::decompress_to_vec_zlib(&packed).map(|t| String::from_utf8_lossy(&t).into_owned()).unwrap_or_default()
    } else {
        String::from_utf8_lossy(body).into_owned()
    };
    let req = form(&text);
    log!("allnet: {path} {}", text.trim());
    let online = std::env::var("WAL_ALLS_ALLNET").is_ok_and(|v| v.trim() == "1");
    let answer = if path.ends_with("/PowerOn") && !online {
        // the cabinet refused (unknown shop): the game plays offline
        Some("stat=-3\n".to_string())
    } else if path.ends_with("/PowerOn") {
        Some(power_on(&req))
    } else if path.ends_with("/DownloadOrder") {
        Some(format!("stat=1&serial={}&uri=null\n", value(&req, "serial")))
    } else {
        None
    };
    let response = match answer {
        Some(text) => {
            log!("allnet: -> {}", text.trim());
            let (body, pragma) = if dfi {
                let packed = miniz_oxide::deflate::compress_to_vec_zlib(text.as_bytes(), 6);
                (format!("{}\r\n", base64_encode(&packed)).into_bytes(), "Pragma: DFI\r\n")
            } else {
                (text.into_bytes(), "")
            };
            let mut r = format!("HTTP/1.1 200 OK\r\nDate: {}\r\nContent-Type: text/plain\r\n{pragma}Content-Length: {}\r\nConnection: close\r\n\r\n", http_date(), body.len()).into_bytes();
            r.extend_from_slice(&body);
            r
        }
        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    };
    stream.write_all(&response)
}

fn serve() {
    match TcpListener::bind(("127.0.0.1", port())) {
        Ok(listener) => {
            log!("allnet: server on 127.0.0.1:{}", port());
            for mut stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let _ = handle(&mut stream);
                });
            }
        }
        Err(e) => log!("allnet: port {}: {e}", port()),
    }
}

/// The server, in this process.
pub fn start() {
    std::thread::spawn(serve);
}

/// The title server in this process: (request path, body) -> answer body.
pub type Title = fn(&str, &[u8]) -> Vec<u8>;

static TITLE: OnceLock<Title> = OnceLock::new();

/// The title server answering in ALL.Net's server (a game without amdaemon: CHUNITHM).
pub fn set_title(title: Title) {
    let _ = TITLE.set(title);
}

static TITLE_DFI: AtomicBool = AtomicBool::new(false);

/// The title server's answers deflated then base64 with `Pragma: DFI` when the request was
/// (Border Break Scramble's client decodes them as ALL.Net's).
pub fn set_title_dfi() {
    TITLE_DFI.store(true, Ordering::Relaxed);
}

static TITLE_RD2: AtomicBool = AtomicBool::new(false);

/// The title server's bodies zlib packed both ways (Shining Force Cross's `NrsHttp`), the
/// answers with `Pragma: DFI` and the packed body's CRC-32 and length (`x-rd2-comp-crc`,
/// `x-rd2-comp-len`; the requests' `x-rd2-crc`), the unpacked body's (`x-rd2-uncomp-crc`,
/// `x-rd2-uncomp-len`), the CRCs in decimal (the game's `_atoi64`).
pub fn set_title_rd2() {
    TITLE_RD2.store(true, Ordering::Relaxed);
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The router's own address is an ALL.Net host (the title server is in ALL.Net's server, here
/// or in amdaemon's).
static ROUTER_IS_ALLNET: AtomicBool = AtomicBool::new(false);

pub fn set_router_is_allnet() {
    ROUTER_IS_ALLNET.store(true, Ordering::Relaxed);
}

/// ALL.Net's hosts (and the addresses the payload's DNS gives them; the router, the title
/// server's host, with a title server here).
fn is_allnet(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let router = (TITLE.get().is_some() || ROUTER_IS_ALLNET.load(Ordering::Relaxed)) && host == dns::dotted(dns::router());
    // the game's own title server host (`WAL_ALLS_TITLE_HOST`, crossbeats REV.'s
    // `rev-ent.ac.capcom.jp`), with a title server here
    let title_host = TITLE.get().is_some()
        && std::env::var("WAL_ALLS_TITLE_HOST").is_ok_and(|v| v.split(',').any(|h| h.trim().eq_ignore_ascii_case(&host)));
    host.ends_with("naominet.jp") || host.ends_with("sys-all.net") || host == "127.0.0.1" || host == "localhost" || router || title_host
}

static CONNECT: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn win_http_connect(session: usize, server: *const u16, port_: u16, reserved: u32) -> usize {
    type F = unsafe extern "system" fn(usize, *const u16, u16, u32) -> usize;
    let orig: F = unsafe { std::mem::transmute(CONNECT.load(Ordering::Relaxed)) };
    let host = if server.is_null() {
        String::new()
    } else {
        let len = (0..).take_while(|&i| unsafe { *server.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(server, len) })
    };
    if is_allnet(&host) {
        log!("allnet: {host}:{port_} -> the payload's server");
        let local: Vec<u16> = "127.0.0.1".encode_utf16().chain(Some(0)).collect();
        return unsafe { orig(session, local.as_ptr(), port(), reserved) };
    }
    unsafe { orig(session, server, port_, reserved) }
}

/// WinHTTP's connections to ALL.Net's hosts: the payload's server.
pub fn hook_winhttp() {
    match unsafe { iat::hook("winhttp.dll", "WinHttpConnect", win_http_connect as *const () as usize) } {
        Some(orig) => CONNECT.store(orig, Ordering::Relaxed),
        None => log!("allnet: WinHttpConnect not imported"),
    }
}

/// A `sockaddr_in` to connect to instead (`None`: the address itself).
pub type Redirect = fn(&[u8; 16]) -> Option<[u8; 16]>;

static REDIRECT: OnceLock<Redirect> = OnceLock::new();

/// The virtual router (ALL.Net's hosts in the payload's DNS): its port 80 the payload's ALL.Net
/// server, 22345 its AimeDB ([`crate::aimedb`]).
pub fn to_server(a: &[u8; 16]) -> Option<[u8; 16]> {
    if a[0] != 2 || a[4..8] != dns::router() {
        return None;
    }
    let to = match u16::from_be_bytes([a[2], a[3]]) {
        80 => port(),
        22345 => crate::aimedb::port(),
        // the billing server, when it runs here ([`crate::billing`])
        8443 if crate::billing::running() => crate::billing::port(),
        _ => return None,
    };
    let mut m = *a;
    m[2..4].copy_from_slice(&to.to_be_bytes());
    m[4..8].copy_from_slice(&[127, 0, 0, 1]);
    Some(m)
}

fn moved(address: *const u8, length: i32) -> Option<[u8; 16]> {
    if address.is_null() || length < 16 {
        return None;
    }
    let a: [u8; 16] = unsafe { std::slice::from_raw_parts(address, 16) }.try_into().ok()?;
    REDIRECT.get()?(&a)
}

static SOCKET_CONNECT: AtomicUsize = AtomicUsize::new(0);
static WSA_CONNECT: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn socket_connect(socket: usize, address: *const u8, length: i32) -> i32 {
    let real: unsafe extern "system" fn(usize, *const u8, i32) -> i32 = unsafe { std::mem::transmute(SOCKET_CONNECT.load(Ordering::Relaxed)) };
    match moved(address, length) {
        Some(m) => unsafe { real(socket, m.as_ptr(), length) },
        None => unsafe { real(socket, address, length) },
    }
}

unsafe extern "system" fn wsa_connect(socket: usize, address: *const u8, length: i32, a: usize, b: usize, c: usize, d: usize) -> i32 {
    type F = unsafe extern "system" fn(usize, *const u8, i32, usize, usize, usize, usize) -> i32;
    let real: F = unsafe { std::mem::transmute(WSA_CONNECT.load(Ordering::Relaxed)) };
    match moved(address, length) {
        Some(m) => unsafe { real(socket, m.as_ptr(), length, a, b, c, d) },
        None => unsafe { real(socket, address, length, a, b, c, d) },
    }
}

/// The game's socket connections (ws2_32's `connect`, by ordinal 4 or by name, and
/// `WSAConnect`) through `redirect`.
pub fn redirect_connect(redirect: Redirect) {
    let _ = REDIRECT.set(redirect);
    unsafe {
        let orig = iat::hook_ordinal(iat::game_module(), "WS2_32.dll", 4, socket_connect as *const () as usize)
            .or_else(|| iat::hook("ws2_32.dll", "connect", socket_connect as *const () as usize));
        if let Some(orig) = orig {
            SOCKET_CONNECT.store(orig, Ordering::Relaxed);
        }
        if let Some(orig) = iat::hook("ws2_32.dll", "WSAConnect", wsa_connect as *const () as usize) {
            WSA_CONNECT.store(orig, Ordering::Relaxed);
        }
    }
}
