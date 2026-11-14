//! ALL.Net for the ALLS cabinet: the shared server ([`wal_payload_common::allnet`]) in amdaemon
//! (its WinHTTP and socket connections to ALL.Net's hosts redirected there), and the cabinet's
//! other servers behind the virtual router's ports ([`moved`]): the game server (`gs.rs`),
//! AimeDB (`wal_payload_common::aimedb`), Initial D ARCADE STAGE Zero's (`idz.rs`), the billing server
//! (`wal_payload_common::billing`).

use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::{allnet, log};

/// The billing server runs in this process (`billing.rs`).
static BILLING: AtomicBool = AtomicBool::new(false);

/// The billing server in this process (a game without amdaemon checks in itself).
pub(crate) fn serve_billing() {
    BILLING.store(true, Ordering::Relaxed);
    wal_payload_common::billing::init();
}

/// The game's process: its connections to the router's port 80 go to its game server (`gs.rs`).
static IN_GAME: AtomicBool = AtomicBool::new(false);

/// A `sockaddr_in` to the virtual router (ALL.Net's hosts in the payload's DNS) or the cabinet's
/// own address: the copy to the payload's server behind it.
fn moved(a: &[u8; 16]) -> Option<[u8; 16]> {
    // the cabinet's own address (absent from the machine): the loopback address, same port
    // (Initial D ARCADE STAGE Zero's store server, `ServerBoxD8_Nu_x64` beside the game)
    if a[0] == 2 && a[4..8] == crate::netenv::iface() {
        let mut m = *a;
        m[4..8].copy_from_slice(&[127, 0, 0, 1]);
        return Some(m);
    }
    if a[0] != 2 || a[4..8] != crate::netenv::router() {
        return None;
    }
    // port 80: ALL.Net (the payload's server; in the game: its game server, `gs.rs`), 22345:
    // AimeDB (`wal_payload_common::aimedb`)
    let to = match u16::from_be_bytes([a[2], a[3]]) {
        80 if IN_GAME.load(Ordering::Relaxed) => crate::gs::port(),
        80 => allnet::port(),
        22345 => wal_payload_common::aimedb::port(),
        // Initial D ARCADE STAGE Zero's game server (`idz.rs`)
        10000 if IN_GAME.load(Ordering::Relaxed) => crate::idz::port(),
        // the billing server (a game checking in itself: CHUNITHM)
        8443 if BILLING.load(Ordering::Relaxed) => wal_payload_common::billing::port(),
        other => {
            log!("allnet: connection to the router's port {other}");
            return None;
        }
    };
    let mut m = *a;
    m[2..4].copy_from_slice(&to.to_be_bytes());
    m[4..8].copy_from_slice(&[127, 0, 0, 1]);
    Some(m)
}

/// The title server is in amdaemon's ALL.Net server (CHUNITHM SUPERSTAR): the game's
/// connections to the router's port 80, its WinHTTP ones to the router too, go there.
pub(crate) fn init_title_redirect() {
    allnet::set_router_is_allnet();
    allnet::redirect_connect(moved);
    allnet::hook_winhttp();
}

/// The redirection of ALL.Net's connections only (the game's process: the server is
/// amdaemon's; port 80: the game's own game server, `gs.rs`).
pub(crate) fn init_redirect() {
    IN_GAME.store(true, Ordering::Relaxed);
    allnet::redirect_connect(moved);
}

/// amdaemon (or a game without it): ALL.Net's server, `{gs}` the game server.
pub(crate) fn init() {
    allnet::set_game_server(format!("127.0.0.1:{}", crate::gs::port()));
    allnet::redirect_connect(moved);
    allnet::hook_winhttp();
    allnet::start();
}
