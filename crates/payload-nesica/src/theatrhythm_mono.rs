//! Theatrhythm's managed code, changed where the cabinet's network is out of reach: the card
//! server is the payload's own, plain HTTP (the game asks for HTTPS with the cabinet's client
//! certificate), and nothing answers as the matching server (Photon), checked every 30 seconds
//! (offline when it does not answer).
//!
//! Replaced methods (Mono's compiled code jumps to the payload's, `wal_payload_common::mono`):
//! * `Siva.Network.ServerManager.SetProtcol`: does nothing, the protocol stays "http://";
//! * `Siva.Network.PhotonManager.Connect`: the status `Online` at once for the server checks
//!   (boot, attract: no lobby option), `Standalone` for a credit's play (`GameStart`: a lobby
//!   option, even empty): its tracks are played alone, without waiting for a room;
//! * `Siva.Network.PhotonManager.Disconnect`: does nothing.

use std::ffi::c_void;

use wal_payload_common::log;
use wal_payload_common::mono::{self, mono};

type P = *mut c_void;

/// PhotonStatus
const PHOTON_STANDALONE: i32 = 0;
const PHOTON_ONLINE: i32 = 2;

/// `PhotonManager.<Status>k__BackingField`'s address.
static PHOTON_STATUS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Called on Unity's main thread (the game's `NESYS_INIT`), before the game uses the network.
pub(crate) fn patch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(m) = mono() else {
            log!("theatrhythm: Mono not found: the card server cannot be used");
            return;
        };
        let image = m.image("Assembly-CSharp");
        let server = m.class(image, "Siva.Network", "ServerManager");
        let photon = m.class(image, "Siva.Network", "PhotonManager");
        let status = m.static_addr(photon, m.field(photon, "<Status>k__BackingField"));
        PHOTON_STATUS.store(status as usize, std::sync::atomic::Ordering::Relaxed);
        let replaced = [
            ("ServerManager.SetProtcol", m.replace(m.method(server, "SetProtcol", 1), set_protcol as *const () as usize)),
            ("PhotonManager.Connect", !status.is_null() && m.replace(m.method(photon, "Connect", 4), photon_connect as *const () as usize)),
            ("PhotonManager.Disconnect", m.replace(m.method(photon, "Disconnect", 0), photon_disconnect as *const () as usize)),
        ];
        for (name, done) in replaced {
            if !done {
                log!("theatrhythm: {name} not replaced");
            }
        }
        log!("theatrhythm: card server over HTTP, matching server always online");
    });
}

/// The protocol stays "http://" (the payload's card server).
unsafe extern "system" fn set_protcol(_protcol: P) {}

/// The matching server is online for the checks; a play is alone.
unsafe extern "system" fn photon_connect(_server: P, _cabinet: P, _lobby: P, option: P) -> i32 {
    let status = PHOTON_STATUS.load(std::sync::atomic::Ordering::Relaxed) as *mut u8;
    let check = option.is_null();
    unsafe { mono::write(status, 0, if check { PHOTON_ONLINE } else { PHOTON_STANDALONE }) };
    check as i32
}

unsafe extern "system" fn photon_disconnect() {}
