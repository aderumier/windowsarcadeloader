//! The game servers of Bandai Namco's network (ALL.Net game servers), run in the game (or its
//! launcher's) process by the Namco ES3 payloads: TLS with a self-signed certificate made at
//! start, HTTP/1.1, protocol buffers.
//!
//! * `wmmt5`: Wangan Midnight Maximum Tune 5 / 5DX / 5DX+ / 6 / 6R / 6RR (`rpc.rs`, its players'
//!   data in a directory: `store.rs`, records: `records.rs`).
//! * `graphene`: Tekken 7 (the Graphene framework, `graphene.protobuf`), and Pokkén Tournament
//!   on the same framework (`jackal.protobuf`, `jackal.rs`).
//! * `spp`: Time Crisis 5 (`spp.net`).

pub mod graphene;
mod jackal;
pub mod pb;
mod records;
mod rpc;
pub mod spp;
mod store;
pub mod wmmt5;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};

pub(crate) static LOGGER: OnceLock<fn(&str)> = OnceLock::new();
pub(crate) static VERBOSE: OnceLock<bool> = OnceLock::new();

pub(crate) fn log(line: &str) {
    if let Some(f) = LOGGER.get() {
        f(line);
    }
}

pub(crate) fn verbose() -> bool {
    VERBOSE.get().copied().unwrap_or(false)
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

/// A server's answer to a request (method, target, body), `None`: not found.
pub(crate) type Route = fn(&str, &str, &[u8]) -> Option<Vec<u8>>;

/// A websocket server's answer to a text message (Pokkén Tournament's admission).
pub(crate) static WEBSOCKET: OnceLock<fn(&str) -> String> = OnceLock::new();

/// One connection: TLS (a record's first byte, 0x16), else plain HTTP (a client whose TLS the
/// payload replaced by plaintext, Pokkén Tournament's axTLS), then [`http`].
pub(crate) fn serve(mut tcp: TcpStream, config: Arc<rustls::ServerConfig>, route: Route, content_type: &str) -> Result<(), String> {
    let mut first = [0u8; 1];
    let n = tcp.peek(&mut first).map_err(|e| e.to_string())?;
    if n == 1 && first[0] != 0x16 {
        // the plaintext client reads its answer until the connection closes
        return http(tcp, route, content_type, true);
    }
    let mut conn = rustls::ServerConnection::new(config).map_err(|e| e.to_string())?;
    http(rustls::Stream::new(&mut conn, &mut tcp), route, content_type, false)
}

/// HTTP/1.1 requests until the connection closes, answered by `route` (HTTP 200,
/// `content_type`, with a `Content-Length`); a websocket upgrade goes to [`WEBSOCKET`]. `close`:
/// one request, its answer, then the connection closed (`Connection: close`).
fn http<S: Read + Write>(stream: S, route: Route, content_type: &str, close: bool) -> Result<(), String> {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
        let (mut length, mut expect, mut ws_key) = (0usize, false, None);
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
                    "sec-websocket-key" => ws_key = Some(value.trim().to_string()),
                    _ => {}
                }
            }
        }
        if let (Some(key), Some(handler)) = (ws_key, WEBSOCKET.get()) {
            return websocket(reader, &key, *handler);
        }
        if expect {
            reader.get_mut().write_all(b"HTTP/1.1 100 Continue\r\n\r\n").map_err(|e| e.to_string())?;
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        let response = match route(&method, &target, &body) {
            Some(body) => {
                let connection = if close { "Connection: close\r\n" } else { "" };
                let mut r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\n{connection}\r\n",
                    body.len()
                )
                .into_bytes();
                r.extend_from_slice(&body);
                r
            }
            None => {
                log(&format!("server: {method} {target}: not found"));
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()
            }
        };
        let stream = reader.get_mut();
        stream.write_all(&response).map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
        if close {
            return Ok(());
        }
    }
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The websocket key's accept value (RFC 6455).
fn websocket_accept(key: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes());
    base64(digest.as_ref())
}

/// A websocket after its upgrade: each text message answered by `handler`, until the client
/// closes it.
fn websocket<S: Read + Write>(mut reader: BufReader<S>, key: &str, handler: fn(&str) -> String) -> Result<(), String> {
    let upgrade = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        websocket_accept(key)
    );
    reader.get_mut().write_all(upgrade.as_bytes()).map_err(|e| e.to_string())?;
    loop {
        let mut head = [0u8; 2];
        reader.read_exact(&mut head).map_err(|e| e.to_string())?;
        let opcode = head[0] & 0x0f;
        let masked = head[1] & 0x80 != 0;
        let mut len = (head[1] & 0x7f) as u64;
        if len == 126 {
            let mut b = [0u8; 2];
            reader.read_exact(&mut b).map_err(|e| e.to_string())?;
            len = u16::from_be_bytes(b) as u64;
        } else if len == 127 {
            let mut b = [0u8; 8];
            reader.read_exact(&mut b).map_err(|e| e.to_string())?;
            len = u64::from_be_bytes(b);
        }
        let mut mask = [0u8; 4];
        if masked {
            reader.read_exact(&mut mask).map_err(|e| e.to_string())?;
        }
        let mut payload = vec![0u8; len.min(1 << 20) as usize];
        reader.read_exact(&mut payload).map_err(|e| e.to_string())?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        let reply = match opcode {
            // close
            0x8 => return Ok(()),
            // ping: pong
            0x9 => (0xA, payload),
            0x1 => (0x1, handler(&String::from_utf8_lossy(&payload)).into_bytes()),
            _ => continue,
        };
        let mut frame = vec![0x80 | reply.0];
        match reply.1.len() {
            n if n < 126 => frame.push(n as u8),
            n if n < 65536 => {
                frame.push(126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&reply.1);
        let stream = reader.get_mut();
        stream.write_all(&frame).map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn websocket_accept_rfc6455() {
        assert_eq!(super::websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert_eq!(super::base64(b"ab"), "YWI=");
    }
}
