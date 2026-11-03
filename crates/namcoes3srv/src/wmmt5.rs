//! The Wangan Midnight Maximum Tune 5 / 5DX / 5DX+ / 6 / 6R / 6RR game server: the cabinet's
//! `https://<server>` (the ALL.Net answer's uri, `127.0.0.1:9002`).
//!
//! The game posts `/method/<rpc>` (a `wm5.protobuf.<Rpc>Request`, `Content-Type:
//! application/x-protobuf; revision=1290`) and gets the `<Rpc>Response` (HTTP 200, the same
//! content type); it gets `/resource/<name>?<query>` (place list, ghosts, rankings).
//!
//! The answers follow Bayshore (Project Asakura's server, its WMMT5 fork Bayshore_5,
//! https://github.com/shadany7824/Bayshore_5; 5DX to 6R: Bayshore_legacy's branches,
//! https://github.com/shiroikitsu8/Bayshore_legacy; 6R: https://github.com/anthonyng43/Bayshore_6r;
//! 6RR: https://github.com/JoeKoh27/Bayshore_6RR): `rpc.rs`. The players' data is kept in a
//! directory (`store.rs`).

use std::net::TcpListener;
use std::sync::Arc;

use crate::{LOGGER, VERBOSE, log, rpc, serve, store, tls_config};

pub use crate::rpc::Game;

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

