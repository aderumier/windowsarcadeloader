//! ALL.Net, the cabinet's authentication server amdaemon reaches with WinHTTP at boot
//! (`http://naominet.jp/sys/servlet/PowerOn`): answered by the payload so the power-on test
//! passes at once (formats from ARTEMiS' `core/allnet.py`, WTFPL).
//!
//! amdaemon's connections to an ALL.Net host (its sockets to port 80 of the virtual router, the
//! address the payload's DNS gives these hosts; `WinHttpConnect`) go to the payload's server on
//! 127.0.0.1:`WAL_PORT`+102. A request's body is a form (`key=value&...`), packed when its
//! `Pragma` header is `DFI` (zlib, then base64; the answer packed the same way):
//! - `PowerOn`: refused (`stat=-3`, the game plays offline, at once); with
//!   `WAL_ALLS_ALLNET=1` accepted (`stat=1`, the shop's values, the title server's URI: the
//!   payload has no title server, and the keychip's billing then stops the game with error
//!   8114, "ALL.Net System error (RTC)")
//! - `DownloadOrder`: no update (`uri=null`)
//!
//! A title server in this process ([`set_title`]: CHUNITHM's, a game with no amdaemon): the
//! requests outside `/sys/servlet/` are its, the game reaching the title uri's host (the router,
//! `{router}` in `WAL_ALLS_TITLE_URI`) through WinHTTP too.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

fn port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 102
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
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

fn base64_decode(text: &[u8]) -> Vec<u8> {
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

fn power_on(req: &[(String, String)]) -> String {
    let format = value(req, "format_ver");
    let r = crate::netenv::router();
    let host = format!("{}.{}.{}.{}", r[0], r[1], r[2], r[3]);
    // the title server's URI (`WAL_ALLS_TITLE_URI`: an ABAAS game's GS server name, a bare host;
    // `{gs}` the payload's game server itself, 127.0.0.1:`WAL_PORT`+104: Chrono Regalia's Mono
    // sockets, not in the game module's imports, never reach the router's redirection)
    let gs = format!("127.0.0.1:{}", crate::gs::port());
    let uri = std::env::var("WAL_ALLS_TITLE_URI").ok().filter(|u| !u.trim().is_empty()).map(|u| u.replace("{router}", &host).replace("{gs}", &gs)).unwrap_or_else(|| format!("http://{host}/{}/", value(req, "game_id")));
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
    if let Some(title) = TITLE.get().filter(|_| !path.contains("/sys/servlet/")) {
        let body = title(&path, body);
        let mut r = format!("HTTP/1.1 200 OK\r\nDate: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", http_date(), body.len()).into_bytes();
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

/// The billing server runs in this process (`billing.rs`).
static BILLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The billing server in this process (a game without amdaemon checks in itself).
pub(crate) fn serve_billing() {
    BILLING.store(true, Ordering::Relaxed);
    crate::billing::init();
}

/// The title server in this process: (request path, body) -> answer body.
pub(crate) type Title = fn(&str, &[u8]) -> Vec<u8>;

static TITLE: std::sync::OnceLock<Title> = std::sync::OnceLock::new();

/// The title server answering in ALL.Net's server (a game without amdaemon: CHUNITHM).
pub(crate) fn set_title(title: Title) {
    let _ = TITLE.set(title);
}

static CONNECT: AtomicUsize = AtomicUsize::new(0);

/// ALL.Net's hosts (and the addresses the payload's DNS gives them; the router, the title
/// server's host, with a title server here).
fn is_allnet(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let r = crate::netenv::router();
    let router = (TITLE.get().is_some() || TITLE_IN_DAEMON.load(Ordering::Relaxed)) && host == format!("{}.{}.{}.{}", r[0], r[1], r[2], r[3]);
    host.ends_with("naominet.jp") || host.ends_with("sys-all.net") || host == "127.0.0.1" || host == "localhost" || router
}

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

static SOCKET_CONNECT: AtomicUsize = AtomicUsize::new(0);
static WSA_CONNECT: AtomicUsize = AtomicUsize::new(0);

/// A `sockaddr_in` to port 80 of the virtual router (ALL.Net's hosts in the payload's DNS): the
/// copy to the payload's server.
fn moved(address: *const u8, length: i32) -> Option<[u8; 16]> {
    if address.is_null() || length < 16 {
        return None;
    }
    let a = unsafe { std::slice::from_raw_parts(address, 16) };
    // the cabinet's own address (absent from the machine): the loopback address, same port
    // (Initial D ARCADE STAGE Zero's store server, `ServerBoxD8_Nu_x64` beside the game)
    if a[0] == 2 && a[4..8] == crate::netenv::iface() {
        let mut m = [0u8; 16];
        m.copy_from_slice(a);
        m[4..8].copy_from_slice(&[127, 0, 0, 1]);
        return Some(m);
    }
    if a[0] != 2 || a[4..8] != crate::netenv::router() {
        return None;
    }
    // port 80: ALL.Net (the payload's server; in the game: its game server, `gs.rs`), 22345:
    // AimeDB (`aimedb.rs`)
    let to = match u16::from_be_bytes([a[2], a[3]]) {
        80 if IN_GAME.load(Ordering::Relaxed) => crate::gs::port(),
        80 => port(),
        22345 => crate::aimedb::port(),
        // Initial D ARCADE STAGE Zero's game server (`idz.rs`)
        10000 if IN_GAME.load(Ordering::Relaxed) => crate::idz::port(),
        // the billing server (a game checking in itself: CHUNITHM)
        8443 if BILLING.load(Ordering::Relaxed) => crate::billing::port(),
        other => {
            log!("allnet: connection to the router's port {other}");
            return None;
        }
    };
    let mut m = [0u8; 16];
    m.copy_from_slice(a);
    m[2..4].copy_from_slice(&to.to_be_bytes());
    m[4..8].copy_from_slice(&[127, 0, 0, 1]);
    Some(m)
}

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

/// ws2_32's `connect`, imported by ordinal (4) or by name (Initial D THE ARCADE's amdaemon and
/// game).
unsafe fn hook_socket_connect() {
    let orig = unsafe { iat::hook_ordinal(iat::game_module(), "WS2_32.dll", 4, socket_connect as *const () as usize) }
        .or_else(|| unsafe { iat::hook("ws2_32.dll", "connect", socket_connect as *const () as usize) });
    if let Some(orig) = orig {
        SOCKET_CONNECT.store(orig, Ordering::Relaxed);
    }
}

/// The game's process: its connections to the router's port 80 go to its game server (`gs.rs`).
static IN_GAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The title server is in amdaemon's ALL.Net server (CHUNITHM SUPERSTAR): the game's
/// connections to the router's port 80, its WinHTTP ones to the router too, go there.
static TITLE_IN_DAEMON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn init_title_redirect() {
    TITLE_IN_DAEMON.store(true, Ordering::Relaxed);
    unsafe {
        hook_socket_connect();
        if let Some(orig) = iat::hook("ws2_32.dll", "WSAConnect", wsa_connect as *const () as usize) {
            WSA_CONNECT.store(orig, Ordering::Relaxed);
        }
        if let Some(orig) = iat::hook("winhttp.dll", "WinHttpConnect", win_http_connect as *const () as usize) {
            CONNECT.store(orig, Ordering::Relaxed);
        }
    }
}

/// The redirection of ALL.Net's connections only (the game's process: the server is
/// amdaemon's; port 80: the game's own game server, `gs.rs`).
pub(crate) fn init_redirect() {
    IN_GAME.store(true, Ordering::Relaxed);
    unsafe {
        hook_socket_connect();
        if let Some(orig) = iat::hook("ws2_32.dll", "WSAConnect", wsa_connect as *const () as usize) {
            WSA_CONNECT.store(orig, Ordering::Relaxed);
        }
    }
}

pub(crate) fn init() {
    unsafe {
        hook_socket_connect();
        if let Some(orig) = iat::hook("ws2_32.dll", "WSAConnect", wsa_connect as *const () as usize) {
            WSA_CONNECT.store(orig, Ordering::Relaxed);
        }
    }
    match unsafe { iat::hook("winhttp.dll", "WinHttpConnect", win_http_connect as *const () as usize) } {
        Some(orig) => CONNECT.store(orig, Ordering::Relaxed),
        None => log!("allnet: WinHttpConnect not imported"),
    }
    std::thread::spawn(serve);
}
