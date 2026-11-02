//! A Wangan Midnight Maximum Tune 5 / 5DX / 5DX+ / 6 / 6R / 6RR game server: the cabinet's `https://<server>` (the ALL.Net
//! answer's uri, `127.0.0.1:9002`), run in the game process by its payload.
//!
//! The game posts `/method/<rpc>` (a `wm5.protobuf.<Rpc>Request`, `Content-Type:
//! application/x-protobuf; revision=1290`) and gets the `<Rpc>Response` (HTTP 200, the same
//! content type); it gets `/resource/<name>?<query>` (place list, ghosts, rankings). TLS with a
//! self-signed certificate made at start: the game's certificate check is patched out.
//!
//! The answers follow Bayshore (Project Asakura's server, its WMMT5 fork Bayshore_5,
//! https://github.com/shadany7824/Bayshore_5; 5DX to 6R: Bayshore_legacy's branches,
//! https://github.com/shiroikitsu8/Bayshore_legacy; 6R: https://github.com/anthonyng43/Bayshore_6r;
//! 6RR: https://github.com/JoeKoh27/Bayshore_6RR): `rpc.rs`. The players' data is kept in a
//! directory (`store.rs`).

pub mod pb;
pub mod spp;
pub use rpc::Game;
mod records;
mod rpc;
mod store;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, OnceLock};

static LOGGER: OnceLock<fn(&str)> = OnceLock::new();
static VERBOSE: OnceLock<bool> = OnceLock::new();

pub(crate) fn log(line: &str) {
    if let Some(f) = LOGGER.get() {
        f(line);
    }
}

pub(crate) fn verbose() -> bool {
    VERBOSE.get().copied().unwrap_or(false)
}

/// Starts the server of `game` on `addrs` (the ALL.Net uri's `127.0.0.1:9002`, the store
/// terminal's proxy), the players' data in `data_dir`. `verbose`: every request and answer in
/// the log.
pub fn start(game: Game, addrs: &[String], data_dir: &str, logger: fn(&str), verbose: bool) {
    rpc::set_game(game);
    let _ = LOGGER.set(logger);
    let _ = VERBOSE.set(verbose);
    store::init(data_dir);
    let config = match tls_config() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            log(&format!("wm5srv: TLS: {e}"));
            return;
        }
    };
    for addr in addrs {
        let listener = match TcpListener::bind(addr) {
            Ok(l) => l,
            Err(e) => {
                log(&format!("wm5srv: {addr}: {e}"));
                continue;
            }
        };
        log(&format!("wm5srv: listening on https://{addr}, data in {data_dir}"));
        let config = config.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if verbose {
                    log(&format!("wm5srv: connection from {:?}", stream.peer_addr()));
                }
                let config = config.clone();
                std::thread::spawn(move || {
                    if let Err(e) = serve(stream, config, wm5_route, "application/x-protobuf; revision=1290") {
                        if verbose {
                            log(&format!("wm5srv: connection: {e}"));
                        }
                    }
                });
            }
        });
    }
}

pub(crate) fn tls_config() -> Result<rustls::ServerConfig, String> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).map_err(|e| e.to_string())?;
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS12])
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .map_err(|e| e.to_string())
}

/// The game's requests: `POST /method/<rpc>`, `GET /resource/<name>?<query>`.
fn wm5_route(method: &str, target: &str, body: &[u8]) -> Option<Vec<u8>> {
    if method == "POST" {
        target.strip_prefix("/method/").map(|rpc| rpc::method(rpc, body))
    } else {
        target.strip_prefix("/resource/").map(|res| {
            let (name, query) = res.split_once('?').unwrap_or((res, ""));
            rpc::resource(name, query)
        })
    }
}

/// A server's answer to a request (method, target, body), `None`: not found.
pub(crate) type Route = fn(&str, &str, &[u8]) -> Option<Vec<u8>>;

/// One connection: HTTP/1.1 requests until it closes, answered by `route` (HTTP 200,
/// `content_type`, with a `Content-Length`).
pub(crate) fn serve(mut tcp: TcpStream, config: Arc<rustls::ServerConfig>, route: Route, content_type: &str) -> Result<(), String> {
    let mut conn = rustls::ServerConnection::new(config).map_err(|e| e.to_string())?;
    let tls = rustls::Stream::new(&mut conn, &mut tcp);
    let mut reader = BufReader::new(tls);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
        let (mut length, mut expect) = (0usize, false);
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).map_err(|e| e.to_string())?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                match name.trim().to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap_or(0),
                    "expect" => expect = value.trim().eq_ignore_ascii_case("100-continue"),
                    _ => {}
                }
            }
        }
        if expect {
            reader.get_mut().write_all(b"HTTP/1.1 100 Continue\r\n\r\n").map_err(|e| e.to_string())?;
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        let response = match route(&method, &target, &body) {
            Some(body) => {
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                r.extend_from_slice(&body);
                r
            }
            None => {
                log(&format!("wm5srv: {method} {target}: not found"));
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()
            }
        };
        let stream = reader.get_mut();
        stream.write_all(&response).map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
    }
}
