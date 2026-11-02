//! Time Crisis 5's game server (Bandai Namco's `spp.net` protocol), run in the cabinet
//! launcher's process by its payload: the launcher posts a `spp.net.Request` (raw protocol
//! buffers, header `X-AV0094-Protobuf-Revision: 15`) to `https://<ALL.Net uri>` and parses
//! the body of the answer (its `Content-Length`) as a `spp.net.Response`.
//!
//! Request: 1 type, 2 network type, 3 seq, 4 PCB serial, 5 place id, 7 register PCB, 12 load
//! user (1 card chip id, 2 access code, 6 create user), 13 save data (1 Banapassport id, 2 user
//! data), 14 update user lock, 15 check user lock, 16 ask access code (1 card chip id).
//! Response: 1 type, 2 error, 3 seq, 4 register PCB (1 registered at, 2 expire at, 3 status),
//! 5 load user (1 unlock at, 2 access code, 3 Banapassport id, 4 personal id, 5 user data),
//! 6 check user lock (1 unlock at), 7 ask access code (1 access code).
//!
//! The players' data is the game's own blob (`user_data`), kept as `users/<id>.bin` with the
//! card's access code in `users/<id>.code`; a player's id is made from the access code.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::pb::{Message, Writer};
use crate::log;

const REGISTER_PCB: u64 = 2;
const LOAD_USER: u64 = 7;
const SAVE_DATA: u64 = 8;
const CHECK_USER_LOCK: u64 = 10;
const ASK_ACCESS_CODE: u64 = 11;

const SUCCESS: u64 = 0;
const USER_NOTFOUND_ERROR: u64 = 8;
/// `PCBStatus` INITIALIZED: the launcher is online with it (and REPAIR); with NORMAL (1) it
/// ends in its "PCB status invalid" state (22-02, 05-20, 05-56).
const PCB_STATUS: u64 = 0;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// The access code of the last card asked by its chip id (`ask_access_code`).
static CARD_CODE: OnceLock<String> = OnceLock::new();
/// One request changes the data at a time.
static LOCK: Mutex<()> = Mutex::new(());

/// Starts the server on `addr` (`host:port`), the players' data in `data_dir`. `access_code`:
/// the code of the reader's card, the answer to a card known only by its chip id. `verbose`:
/// every request and answer in the log.
pub fn start(addr: &str, data_dir: &str, access_code: &str, logger: fn(&str), verbose: bool) {
    let _ = crate::LOGGER.set(logger);
    let _ = crate::VERBOSE.set(verbose);
    let dir = PathBuf::from(data_dir).join("users");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log(&format!("spp: {}: {e}", dir.display()));
    }
    let _ = DIR.set(dir);
    let _ = CARD_CODE.set(access_code.to_string());
    let config = match crate::tls_config() {
        Ok(c) => Arc::new(c),
        Err(e) => return log(&format!("spp: TLS: {e}")),
    };
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => return log(&format!("spp: {addr}: {e}")),
    };
    log(&format!("spp: listening on https://{addr}, data in {data_dir}"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let config = config.clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::serve(stream, config, route, "application/octet-stream") {
                    log(&format!("spp: connection: {e}"));
                }
            });
        }
    });
}

fn route(method: &str, _target: &str, body: &[u8]) -> Option<Vec<u8>> {
    if method != "POST" {
        return None;
    }
    let Some(request) = Message::parse(body) else {
        log("spp: request not decoded");
        return None;
    };
    let reply = answer(&request);
    if crate::verbose() {
        log(&format!("spp: request {}", request.dump()));
        log(&format!("spp: answer {}", Message::parse(&reply).unwrap_or_default().dump()));
    }
    Some(reply)
}

/// A player's id: from the card's access code (never 0).
fn user_id(access_code: &str) -> u32 {
    let hash = access_code.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    (hash & 0x7FFF_FFFF).max(1)
}

fn path(id: u32, ext: &str) -> PathBuf {
    DIR.get().cloned().unwrap_or_default().join(format!("{id}.{ext}"))
}

fn answer(request: &Message) -> Vec<u8> {
    let kind = request.uint(1).unwrap_or(0);
    let seq = request.uint(3).unwrap_or(0);
    let _guard = LOCK.lock().unwrap();
    let mut error = SUCCESS;
    let mut r = Writer::new();
    match kind {
        REGISTER_PCB => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            r = r.message(4, Writer::new().uint(1, now).uint(2, now + 365 * 24 * 3600).uint(3, PCB_STATUS));
            log(&format!("spp: cabinet {} registered", request.string(4).unwrap_or_default()));
        }
        LOAD_USER => {
            let user = request.message(12).unwrap_or_default();
            let code = user.string(2).filter(|c| !c.is_empty()).or_else(|| CARD_CODE.get().cloned()).unwrap_or_default();
            let create = user.uint(6).unwrap_or(0) != 0;
            let id = user_id(&code);
            let data = std::fs::read(path(id, "bin")).ok();
            if data.is_none() && !create {
                log(&format!("spp: card {code}: no player"));
                error = USER_NOTFOUND_ERROR;
            } else {
                if data.is_none() {
                    log(&format!("spp: card {code}: new player {id}"));
                    let _ = std::fs::write(path(id, "code"), &code);
                } else {
                    log(&format!("spp: card {code}: player {id}"));
                }
                let mut u = Writer::new().uint(1, 0).string(2, &code).uint(3, id as u64).uint(4, id as u64);
                if let Some(data) = &data {
                    u = u.bytes(5, data);
                }
                r = r.message(5, u);
            }
        }
        SAVE_DATA => {
            let save = request.message(13).unwrap_or_default();
            let id = save.uint(1).unwrap_or(0) as u32;
            let data = save.bytes(2).unwrap_or_default();
            match std::fs::write(path(id, "bin"), data) {
                Ok(()) => log(&format!("spp: player {id} saved ({} bytes)", data.len())),
                Err(e) => log(&format!("spp: player {id}: {e}")),
            }
        }
        CHECK_USER_LOCK => r = r.message(6, Writer::new().uint(1, 0)),
        ASK_ACCESS_CODE => r = r.message(7, Writer::new().string(1, CARD_CODE.get().map_or("", |c| c.as_str()))),
        // the cabinet's bookkeeping, charges, network status, user locks: accepted
        _ => {}
    }
    Writer::new().uint(1, kind).uint(2, error).uint(3, seq).raw(&r.0).0
}
