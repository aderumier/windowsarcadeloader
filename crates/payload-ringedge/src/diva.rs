//! Hatsune Miku Project DIVA Arcade, game id `SBTY` (Sega RingEdge 2, OpenGL; Version B): the
//! devices and the network the base RingEdge payload does not already answer. Enabled by
//! `WAL_RINGEDGE_DIVA=1`.
//!
//! - The game's network ([`allnet`]): its own ALL.Net client, its title server
//!   (payload-common `divaserver.rs`), AimeDB, the shop's local file server found by broadcast ([`sendto`]).
//! - The touch panel: the Elo 2701 controller (payload-common `elo.rs`, `WAL_RINGEDGE_ELO`).
//! - The Aime NFC reader (TN32MSEC003S, `aimelib` 3.03) on `WAL_RINGEDGE_DIVA_AIME` (default
//!   `COM2`): `wal_payload_common::sgreader`, generation 1 (`WAL_AIME_READER=tn32`, set in the
//!   profile). Player 1's `card` input touches it with the player's card; `WAL_RINGEDGE_AUTO_CARD
//!   =1`: after start or a touch, the card is on the reader when the game polls it (until read).
//! - The 4 hit buttons (triangle, square, cross, circle), start, test and service on the JVS
//!   board (`jvs_board`, the base payload), no custom wiring.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{input, log, sgreader};
use wal_protocol::button;

/// Player 1's 20-digit Aime access code (`WAL_RINGEDGE_AIME_CODE`, default the first card).
fn access_code() -> String {
    let code: String = std::env::var("WAL_RINGEDGE_AIME_CODE").unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    if code.len() == 20 { code } else { "50000000000000000001".into() }
}

/// The Aime reader on its port (generation 1 through `WAL_AIME_READER=tn32`), player 1's card
/// input touching it with the player's card.
fn aime(port: &str) {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    sgreader::init(port, &[exe]);
    let code = access_code();
    log!("diva: Aime reader on {port}, card {code}");
    if std::env::var("WAL_RINGEDGE_AUTO_CARD").is_ok_and(|v| v.trim() == "1") {
        // after start, or a touch (a game started on the touch screen)
        sgreader::card_on_poll_after_press(&code, Duration::from_millis(0), |p1| p1.pressed(button::START) || p1.pressed(button::B8));
    }
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let card = input(0).pressed(button::CARD);
            if card && !held {
                sgreader::insert(&code, Duration::from_secs(3));
            }
            held = card;
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

/// The game's own ALL.Net client (`alw`): the shop's router and ALL.Net's hosts are the
/// network's gateway (`wal_payload_common::dns`: at once; the machine's DNS never answered the
/// routers' `.loc` names, the boot stuck at the network check), its connections to port 80 the
/// payload's ALL.Net server (`wal_payload_common::allnet`); it sets the clock to the server's
/// time (refused by Wine: the power-on never ended), the setters doing nothing.
/// Also another RingEdge game's own ALL.Net client (`WAL_RINGEDGE_ALLNET=1`, Border Break
/// Scramble): the same, without DIVA's title server (the router's port 80 is ALL.Net's).
fn allnet(diva: bool) {
    wal_payload_common::clock::hook_setters();
    let ip = crate::pcp::local_address().unwrap_or(std::net::Ipv4Addr::new(192, 168, 0, 2)).octets();
    wal_payload_common::dns::set_router([ip[0], ip[1], ip[2], 1]);
    wal_payload_common::dns::init();
    wal_payload_common::allnet::redirect_connect(wal_payload_common::allnet::to_server);
    if diva {
        // its title server (the power-on's uri, `WAL_ALLS_TITLE_URI`: `/DivaServlet/`)
        wal_payload_common::allnet::set_title(wal_payload_common::divaserver::answer);
    } else if std::env::var("WAL_RINGEDGE_ALLNET").is_ok_and(|v| v.trim() == "borderbreak") {
        // Border Break Scramble's title server (`/SBVA/`)
        wal_payload_common::allnet::set_title(crate::bbserver::answer);
        wal_payload_common::allnet::set_title_dfi();
        // its billing (alpb, ib.naominet.jp:8443): checked in at boot ("Error 21" without)
        wal_payload_common::billing::init();
        crate::bbserver::set_role();
        crate::bbserver::extend_markers();
        crate::bbserver::free_play();
    } else {
        wal_payload_common::allnet::set_router_is_allnet();
    }
    wal_payload_common::allnet::start();
    // its card system's AimeDB (`aime.naominet.jp`, the router's port 22345)
    wal_payload_common::aimedb::init();
}

static SENDTO: AtomicUsize = AtomicUsize::new(0);

/// `sendto` (ws2_32 ordinal 20): a broadcast (`x.x.x.255`, `255.255.255.255`) to the loopback
/// address, same port. The shop's local file server (LFS, the main cabinet's, this one's):
/// its client finds it with a broadcast to the subnet on UDP 30001, which the machine does not
/// deliver back to its own sockets (Location Server stuck at WAIT 9).
unsafe extern "system" fn sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    let real: unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32 = unsafe { std::mem::transmute(SENDTO.load(Ordering::Relaxed)) };
    if !to.is_null() && tolen >= 16 {
        let mut a = [0u8; 16];
        a.copy_from_slice(unsafe { std::slice::from_raw_parts(to, 16) });
        if a[0] == 2 && a[7] == 255 {
            a[4..8].copy_from_slice(&[127, 0, 0, 1]);
            return unsafe { real(s, buf, len, flags, a.as_ptr(), tolen) };
        }
    }
    unsafe { real(s, buf, len, flags, to, tolen) }
}

/// The broadcasts to the loopback address ([`sendto`]).
fn hook_broadcast() {
    let f = sendto as *const () as usize;
    let exe = wal_payload_common::iat::game_module();
    if let Some(o) = unsafe { wal_payload_common::iat::hook_ordinal(exe, "WS2_32.dll", 20, f) }.or_else(|| unsafe { wal_payload_common::iat::hook("ws2_32.dll", "sendto", f) }) {
        SENDTO.store(o, Ordering::Relaxed);
    }
}

/// The game's devices, when `WAL_RINGEDGE_DIVA=1` (after the base payload's hooks).
pub(crate) fn init() {
    if !std::env::var("WAL_RINGEDGE_DIVA").is_ok_and(|v| v.trim() == "1") {
        if std::env::var("WAL_RINGEDGE_ALLNET").is_ok_and(|v| !v.trim().is_empty() && v.trim() != "0") {
            allnet(false);
            // Border Break Scramble's LFS is found the same way
            hook_broadcast();
        }
        return;
    }
    allnet(true);
    hook_broadcast();
    let aime_port = std::env::var("WAL_RINGEDGE_DIVA_AIME").ok().map(|p| p.trim().to_ascii_uppercase()).unwrap_or_else(|| "COM2".into());
    if aime_port.starts_with("COM") {
        aime(&aime_port);
    }
}
