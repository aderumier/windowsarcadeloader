//! A local card server that answers a Banapass card with a fully unlocked player profile
//! (every kart, character and course), for offline play.
//!
//! The game's card-server client sends `GET /<region>/<api>/<function>` requests and reads
//! JSON-like arrays back; only the four calls a card login makes need an answer, every other
//! call gets `[0]` (success, no data). The profile data comes from the MKDX-Unlocked-Profile
//! project (Harm). `WAL_NAMCOES3_PROFILE_NAME` is the player name shown in game.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use wal_payload_common::log;

/// `player/getData`: the unlocked profile, `{NAME}` replaced by the player name.
const PLAYER: &str = include_str!("mkdx_player.json");

/// Starts the server on `addr` (`host:port`) in the background.
pub(crate) fn start(addr: String) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                log!("profile: cannot listen on {addr}: {e}");
                return;
            }
        };
        log!("profile: unlocked profile server on http://{addr}/");
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
}

fn name() -> String {
    let name = std::env::var("WAL_NAMCOES3_PROFILE_NAME").unwrap_or_default();
    let name: String = name.trim().chars().filter(|c| *c != '"' && *c != '\\' && !c.is_control()).take(32).collect();
    if name.is_empty() { "Unlocked".into() } else { name }
}

/// The answer to one request path.
fn respond(path: &str) -> String {
    let path = path.split('?').next().unwrap_or("");
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 4 {
        return "[0]".into();
    }
    match (parts[2], parts[3]) {
        ("server", "getStatus") => "[0,9]".into(),
        ("amid", "getAmidInfo_bf") => {
            r#"[0,0,1,1234567890,1007,"JPN",1234567890,"JPN",0,0,1,0,"","","","",""]"#.into()
        }
        ("player", "getData") => PLAYER.trim_end().replace("{NAME}", &name()),
        ("judgement", "getData") => concat!(
            r#"[0,[[[0,0,3]],[[0,2],[10,1]],[[18,3],[22,1],[39,1],[41,2],[56,2]],"#,
            r#"[[1,0,7,2],[2,0,7,2],[10,0,3,6],[11,0,5,5],[15,0,2,0],[16,0,1,0]],"#,
            r#""66666666666666666666666666666666666666666666666666"]]"#,
        )
        .into(),
        _ => "[0]".into(),
    }
}

/// One HTTP/1.1 exchange: read the request head (and any body), answer, close.
fn serve(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("");
    let length: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    // Drain the request body so the client sees the whole request consumed.
    let mut have = buf.len() - head_end;
    while have < length.min(1 << 20) {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => have += n,
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let body = if path == "/" { "oh hi :)".to_string() } else { respond(path) };
    log!("profile: {request} -> {} bytes", body.len());
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}
