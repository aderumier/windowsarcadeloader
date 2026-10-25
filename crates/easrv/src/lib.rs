//! A fake e-Amusement server (`WAL_KONAMI_EASRV=1`): the games whose boot needs Konami's network
//! (Steel Chronicle Victroopers: 5-2000-0000 otherwise) get its services from this one, on
//! `127.0.0.1:33701`. wal-avs-boot runs it (before the game's e-Amusement boot, which asks for the
//! services at once) and points the game's `/ea3/network/services` at it; Winning Eleven 2008's
//! payload does both itself (the game boots e-Amusement).
//!
//! The game posts `<model>/<module>/<method>` (or `?model=..&module=..&method=..`, or
//! `?model=..&f=<module>.<method>`, or `/` with the call naming them) requests: binary or text
//! XML (`kbin.rs`), RC4-encrypted with the key of its `X-Eamuse-Info` header, maybe
//! LZ77-compressed (`X-Compress: lz77`). The answers are built as binary XML, encrypted when the
//! request is (for spice2x's `X-Eamuse-Info: 1-53d121c7-a8b3`):
//! - `easrv/answers.rs`: spice2x's (easrv.cpp, the open-source reference): the services list
//!   (every service here), the PCB tracker, messages, the facility, package list, tax phase,
//!   event log, machine control, Steel Chronicle's `system.getmaster` / `hdkoperation.get`,
//!   Metal Gear Arcade's `system.getmaster`, Winning Eleven 2008's services list;
//! - `easrv/hck.rs`: Winning Eleven 2008's own modules (settings, its players' records, login);
//! - `easrv/jmp.rs`: Hello! Pop'n Music's `game` module (Asphyxia CORE's popn-hello plugin);
//! - `easrv/cards.rs`: the card manager (cards, PINs: `dev/nvram/easrv/`);
//! - `easrv/playerdata.rs`: the players' records (`playerdata.usergamedata_*`), per card;
//! - anything else an empty answer.
//!
//! `WAL_KONAMI_EASRV_LOG=1`: every request and answer, as XML text, in `easrv.log` (the run
//! directory).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::kbin::Node;

pub const PORT: u16 = 33701;

/// Where the server's log lines go (the host's log; standard error by default).
static LOGGER: OnceLock<fn(&str)> = OnceLock::new();

/// Sends the server's log lines to `logger` (before [`init`]).
pub fn set_logger(logger: fn(&str)) {
    let _ = LOGGER.set(logger);
}

pub(crate) fn log_line(line: &str) {
    match LOGGER.get() {
        Some(f) => f(line),
        None => eprintln!("{line}"),
    }
}

macro_rules! log {
    ($($t:tt)*) => { $crate::log_line(&format!($($t)*)) };
}
pub(crate) use log;

mod answers;
mod cards;
mod hck;
mod jmp;
pub mod kbin;
mod playerdata;

/// This server's services URL (`/` and `localhost` are refused by the game's e-Amusement
/// library: "bad URI", "bad services host").
fn url() -> String {
    format!("http://127.0.0.1:{PORT}/service/")
}

/// The encrypted answers' key header (spice2x's).
const EAMUSE_INFO: &str = "1-53d121c7-a8b3";

/// The fixed answers, `(name, document)`, built once.
fn fixed() -> &'static [(&'static str, Vec<u8>)] {
    static FIXED: OnceLock<Vec<(&'static str, Vec<u8>)>> = OnceLock::new();
    FIXED.get_or_init(|| answers::answers(&url()))
}

/// Requests logged in the payload log (the first ones of each run).
static LOGGED: AtomicUsize = AtomicUsize::new(0);
static XML_LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn xml_log(text: &str) {
    if let Some(f) = XML_LOG.lock().unwrap().as_mut() {
        let _ = f.write_all(text.as_bytes());
    }
}

/// `(model, module, method)` of a request's path.
fn request(path: &str) -> (String, String, String) {
    let query = |key: &str| {
        path.split(['?', '&']).find_map(|p| p.strip_prefix(key).and_then(|v| v.strip_prefix('='))).unwrap_or_default().to_string()
    };
    if path.contains("module=") {
        return (query("model"), query("module"), query("method"));
    }
    // the newer form: `?model=..&f=<module>.<method>`
    if path.contains("f=") {
        let f = query("f");
        let (module, method) = f.split_once('.').unwrap_or((&f, ""));
        return (query("model"), module.to_string(), method.to_string());
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        [.., model, module, method] => (model.to_string(), module.to_string(), method.to_string()),
        _ => Default::default(),
    }
}

/// `(model, module, method)` of a request's `<call model=..><module method=../></call>`.
fn body_request(call: &Node) -> (String, String, String) {
    let attr = |n: &Node, key: &str| n.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default();
    match call.children.first() {
        Some(module) => (attr(call, "model"), module.name.clone(), attr(module, "method")),
        None => Default::default(),
    }
}

/// The answer document and how it was found (for the log).
fn answer(model: &str, module: &str, method: &str, req: Option<&Node>) -> (Vec<u8>, &'static str) {
    if model.starts_with("HCK")
        && let Some(answer) = hck::handle(module, method, req)
    {
        return (Node::new("response").child(answer).to_binary(), "HCK");
    }
    if model.starts_with("JMP")
        && let Some(answer) = jmp::handle(module, method, req)
    {
        return (Node::new("response").child(answer).to_binary(), "JMP");
    }
    // Metal Gear Arcade's server messages: `INFO` (the unlocks, the fixed answer below); the
    // others (`MAPPAT`, `RK_TERM`) none, `result` 0 (bemaniutils' answer)
    if model.starts_with("I36")
        && module == "system"
        && method == "getmaster"
        && req.and_then(|r| r.path("system/data/datakey")).is_some_and(|k| k.text() != "INFO")
    {
        return (Node::new("response").child(Node::new("system").child(Node::value("result", kbin::Value::S32(0)))).to_binary(), "I36 getmaster");
    }
    if module == "cardmng" {
        return (Node::new("response").child(cards::handle(method, req)).to_binary(), "cardmng");
    }
    if module == "playerdata" {
        return (Node::new("response").child(playerdata::handle(method, req)).to_binary(), "playerdata");
    }
    if module == "group" && method == "getid" {
        return (Node::new("response").child(cards::group_getid(req)).to_binary(), "group.getid");
    }
    if module == "system" && method == "convcardnumber" {
        return (Node::new("response").child(cards::convcardnumber(req)).to_binary(), "convcardnumber");
    }
    let name = format!("{module}.{method}");
    let game = format!("{}.{name}", model.get(..3).unwrap_or_default());
    let all = fixed();
    // the game's own answer first
    match all.iter().find(|(n, _)| *n == game).or_else(|| all.iter().find(|(n, _)| *n == name)) {
        Some((n, doc)) => (doc.clone(), *n),
        None => (all.iter().find(|(n, _)| *n == "empty").unwrap().1.clone(), "empty"),
    }
}

/// The header `name`'s value (lower case name).
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (n, v) = l.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// A request body as a document: decrypted (its `X-Eamuse-Info` key), decompressed.
fn decode(head: &str, body: &[u8]) -> Option<Node> {
    let mut body = body.to_vec();
    if let Some(key) = header(head, "x-eamuse-info").and_then(crate::kbin::info_key) {
        crate::kbin::rc4(&key, &mut body);
    }
    if header(head, "x-compress").is_some_and(|c| c.eq_ignore_ascii_case("lz77")) {
        body = crate::kbin::lz77_decompress(&body);
    }
    let node = Node::parse(&body);
    if node.is_none() {
        let hex: String = body.iter().take(96).map(|b| format!("{b:02x}")).collect();
        xml_log(&format!("(not decoded: {} bytes {hex})\n", body.len()));
        let _ = std::fs::write("easrv-undecoded.bin", &body);
    }
    node
}

fn serve(mut stream: TcpStream) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    // the headers, then the body's Content-Length
    let (head_end, length) = loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => {
                xml_log(&format!("#### connection closed after {} bytes: {:?}\n", data.len(), String::from_utf8_lossy(&data)));
                return;
            }
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).into_owned();
            let length = header(&head, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0usize);
            break (end + 4, length);
        }
    };
    while data.len() < head_end + length {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    xml_log(&format!("#### {} bytes\n{head}", data.len()));
    let path = head.split_whitespace().nth(1).unwrap_or_default().to_string();
    // an encrypted request (its X-Eamuse-Info header) wants an encrypted answer
    let crypt = header(&head, "x-eamuse-info").is_some();
    let body: Vec<u8> = if head.starts_with("POST") {
        let req = decode(&head, &data[head_end..(head_end + length).min(data.len())]);
        let (model, module, method) = match request(&path) {
            // the older form (Winning Eleven 2008): `POST /`, the call's child is the module, its
            // `method` attribute the method
            (_, module, _) if module.is_empty() => req.as_ref().map(body_request).unwrap_or_default(),
            r => r,
        };
        let (mut doc, name) = answer(&model, &module, &method, req.as_ref());
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 200 {
            log!("easrv: {module}.{method} -> {name}{}", if req.is_none() { " (request not decoded)" } else { "" });
        }
        let request_text = req.as_ref().map(|r| r.to_text()).unwrap_or_else(|| "(not decoded)\n".into());
        let answer_text = Node::from_binary(&doc).map(|n| n.to_text()).unwrap_or_default();
        xml_log(&format!("==== {module}.{method} ({model})\n{request_text}---- answer ({name})\n{answer_text}\n"));
        if crypt {
            crate::kbin::rc4(&crate::kbin::info_key(EAMUSE_INFO).unwrap(), &mut doc);
        }
        doc
    } else {
        Vec::new()
    };
    let info = if crypt { format!("X-Eamuse-Info: {EAMUSE_INFO}\r\n") } else { String::new() };
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n{info}X-Compress: none\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// Starts the server when `WAL_KONAMI_EASRV=1` and no external server is set
/// (`WAL_KONAMI_SERVICES`, the game's services URL then: wal-avs-boot sets it).
pub fn init() {
    if std::env::var("WAL_KONAMI_EASRV").map(|v| v.trim() != "1").unwrap_or(true) {
        return;
    }
    if std::env::var("WAL_KONAMI_SERVICES").is_ok_and(|v| !v.trim().is_empty()) {
        log!("easrv: external e-Amusement server, the fake one not started");
        return;
    }
    if std::env::var("WAL_KONAMI_EASRV_LOG").is_ok_and(|v| v.trim() == "1") {
        *XML_LOG.lock().unwrap() = std::fs::File::create("easrv.log").ok();
    }
    cards::preregister();
    match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => {
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || serve(stream));
                }
            });
        }
        Err(e) => log!("easrv: port {PORT}: {e}"),
    }
}
